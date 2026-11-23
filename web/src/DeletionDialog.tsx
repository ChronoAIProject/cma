import { refreshSandboxCapacity } from './sandbox/useSandboxCapacity';
import { usePresenceExiting } from './ui/Presence';
import { useEffect, useRef, useState } from 'react';
import type { CmaWebClient } from './api/client';
import { ApiError } from '@cma/client';
import {
  deletionTarget,
  observeDeletion,
  requestDeletion,
  type DeletionObservation,
  type DeletionRequest,
} from './deletion';
import { forgetDeletion, persistDeletion } from './deletionRecovery';
import { RiskConfirmation } from './ui/RiskConfirmation';
import { Modal } from './ui/Modal';
import { Button } from './ui/Button';
import { DeletionImpactSummary } from './DeletionImpactSummary';
import { DeletionStatusPanel } from './DeletionStatusPanel';
import { transitionRefreshDelay } from './sandbox/lifecycle';

export function DeletionDialog({
  client,
  viewerId,
  initial,
  restored,
  onChanged,
  onAccepted,
  backgroundOnAcceptance = false,
  onClose,
}: {
  readonly client: CmaWebClient;
  readonly viewerId: string;
  readonly initial: DeletionRequest;
  readonly restored: boolean;
  readonly onChanged: () => void;
  readonly onAccepted?: (request: DeletionRequest) => void;
  readonly backgroundOnAcceptance?: boolean;
  readonly onClose: () => void;
}) {
  const exiting = usePresenceExiting();
  const [request, setRequest] = useState(initial);
  const [started, setStarted] = useState(restored);
  const [sending, setSending] = useState(false);
  const [observation, setObservation] = useState<DeletionObservation>();
  const [readError, setReadError] = useState<string>();
  const [commandError, setCommandError] = useState<string>();
  const [revision, setRevision] = useState(0);
  const inFlight = useRef(false);
  const backgrounded = useRef(false);
  const changed = useRef(onChanged);
  const completed = useRef(false);
  useEffect(() => {
    changed.current = onChanged;
  }, [onChanged]);
  const target = deletionTarget(request.action);
  const persist = (value: DeletionRequest | undefined) => {
    if (value === undefined) forgetDeletion(viewerId, request.key);
    else persistDeletion(viewerId, value);
  };

  useEffect(() => {
    if (exiting) return;
    backgrounded.current = false;
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let events: DeletionObservation['events'] = [];
    let agentEvents: DeletionObservation['agentEvents'] = [];
    let reading = false;
    const startedAt = Date.now();
    const poll = () => {
      if (
        reading ||
        controller.signal.aborted ||
        document.hidden ||
        completed.current ||
        backgrounded.current
      )
        return;
      clearTimeout(timer);
      reading = true;
      void observeDeletion(
        client,
        request.action,
        controller.signal,
        request.accepted,
        events,
        agentEvents,
      )
        .then((next) => {
          if (controller.signal.aborted) return;
          events = next.events;
          agentEvents = next.agentEvents;
          setObservation(next);
          setReadError(undefined);
          if (next.inProgress && !request.accepted) {
            const accepted = { ...request, accepted: true };
            persistDeletion(viewerId, accepted);
            setRequest(accepted);
            setStarted(true);
            changed.current();
          }
          if (next.complete) {
            completed.current = true;
            if (request.action.kind === 'delete_sandbox')
              void refreshSandboxCapacity(client, viewerId);
            forgetDeletion(viewerId, request.key);
            changed.current();
          } else
            timer = setTimeout(poll, request.accepted ? transitionRefreshDelay(startedAt) : 1500);
        })
        .catch((error: unknown) => {
          if (controller.signal.aborted) return;
          if (error instanceof ApiError && [401, 403, 404].includes(error.status)) {
            events = [];
            agentEvents = [];
            setObservation(undefined);
          }
          // A missing resource can mean access loss, not proven deletion.
          setReadError(
            error instanceof ApiError && [401, 403, 404].includes(error.status)
              ? 'Deletion could not be verified. The resource is no longer accessible with this account.'
              : 'Deletion status could not be refreshed. Reconnecting; completion is not yet confirmed.',
          );
          timer = setTimeout(poll, 5000);
        })
        .finally(() => {
          reading = false;
        });
    };
    document.addEventListener('visibilitychange', poll);
    poll();
    return () => {
      controller.abort();
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', poll);
    };
  }, [client, request, revision, viewerId, exiting]);

  const submit = async () => {
    if (inFlight.current || request.accepted) return;
    if (
      !started &&
      (observation === undefined || readError !== undefined || observation.blocked !== undefined)
    )
      return;
    inFlight.current = true;
    setStarted(true);
    setSending(true);
    setCommandError(undefined);
    persist(request);
    try {
      await requestDeletion(client, request);
      const accepted = { ...request, accepted: true };
      persist(accepted);
      backgrounded.current = backgroundOnAcceptance;
      setRequest(accepted);
      changed.current();
      onAccepted?.(accepted);
    } catch (error: unknown) {
      setCommandError(
        error instanceof Error ? error.message : 'The delete request could not be confirmed.',
      );
      if (error instanceof ApiError && [400, 401, 403, 404, 422].includes(error.status)) {
        persist(undefined);
        setStarted(false);
      }
    } finally {
      inFlight.current = false;
      setSending(false);
    }
  };
  const close = () => {
    if (!started || completed.current) persist(undefined);
    onClose();
  };
  const name = observation?.name ?? target;
  if (!started && !observation?.complete)
    return (
      <RiskConfirmation
        open
        title={`Delete ${target}`}
        description={
          <DeletionImpactSummary
            key={JSON.stringify([viewerId, request.action])}
            client={client}
            viewerId={viewerId}
            action={request.action}
            observation={observation}
            available={observation !== undefined && readError === undefined}
          />
        }
        acknowledgeLabel={`I understand this ${target} will be deleted.`}
        confirmLabel={`Delete ${target}`}
        confirmDisabled={
          observation === undefined || readError !== undefined || observation.blocked !== undefined
        }
        status={observation === undefined ? 'Checking deletion availability.' : observation.blocked}
        failure={commandError ?? readError}
        onCancel={close}
        onConfirm={() => {
          void submit();
        }}
      />
    );
  return (
    <Modal
      open
      title={observation?.complete ? `${target} deleted` : `Deleting ${target}`}
      description={name}
      dismissible={!sending}
      onClose={close}
      footer={
        <>
          <Button disabled={sending} onClick={close}>
            {observation?.complete ? 'Done' : 'Close'}
          </Button>
          {!request.accepted && !observation?.complete ? (
            <Button
              variant="primary"
              disabled={sending}
              onClick={() => {
                void submit();
              }}
            >
              Check request
            </Button>
          ) : null}
          {readError === undefined ? null : (
            <Button onClick={() => setRevision((value) => value + 1)}>Refresh status</Button>
          )}
        </>
      }
    >
      <DeletionStatusPanel
        request={request}
        observation={observation}
        sending={sending}
        commandError={commandError}
        readError={readError}
      />
    </Modal>
  );
}
