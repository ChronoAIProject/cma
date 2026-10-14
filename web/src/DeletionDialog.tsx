import { Presence } from './ui/Presence';
import { useEffect, useRef, useState } from 'react';
import type { CmaWebClient } from './api/client';
import { ApiError } from '@chronoai/cma-sdk';
import {
  deletionStorageKey,
  deletionTarget,
  observeDeletion,
  requestDeletion,
  restoreDeletion,
  type DeletionObservation,
  type DeletionRequest,
} from './deletion';
import { RiskConfirmation } from './ui/RiskConfirmation';
import { Modal } from './ui/Modal';
import { Button } from './ui/Button';
import { ProvisioningProgress } from './sandbox/ProvisioningProgress';
import { LiveActivity } from './sandbox/LiveActivity';
import { activityEntries } from './sandbox/activity';
import { transitionRefreshDelay } from './sandbox/lifecycle';

export function DeletionDialog({
  client,
  viewerId,
  initial,
  restored,
  onChanged,
  onAccepted,
  onClose,
}: {
  readonly client: CmaWebClient;
  readonly viewerId: string;
  readonly initial: DeletionRequest;
  readonly restored: boolean;
  readonly onChanged: () => void;
  readonly onAccepted?: (request: DeletionRequest) => void;
  readonly onClose: () => void;
}) {
  const [request, setRequest] = useState(initial);
  const [started, setStarted] = useState(restored);
  const [sending, setSending] = useState(false);
  const [observation, setObservation] = useState<DeletionObservation>();
  const [readError, setReadError] = useState<string>();
  const [commandError, setCommandError] = useState<string>();
  const [revision, setRevision] = useState(0);
  const inFlight = useRef(false);
  const changed = useRef(onChanged);
  const completed = useRef(false);
  useEffect(() => {
    changed.current = onChanged;
  }, [onChanged]);
  const target = deletionTarget(request.action);
  const persist = (value: DeletionRequest | undefined) => {
    try {
      if (value === undefined) {
        if (restoreDeletion(viewerId)?.key === request.key)
          localStorage.removeItem(deletionStorageKey(viewerId));
      } else localStorage.setItem(deletionStorageKey(viewerId), JSON.stringify(value));
    } catch {
      /* The server still owns the operation when browser storage is unavailable. */
    }
  };

  useEffect(() => {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let events: DeletionObservation['events'] = [];
    let agentEvents: DeletionObservation['agentEvents'] = [];
    let reading = false;
    const startedAt = Date.now();
    const poll = () => {
      if (reading || controller.signal.aborted || document.hidden || completed.current) return;
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
          if (next.complete) {
            completed.current = true;
            try {
              if (restoreDeletion(viewerId)?.key === request.key)
                localStorage.removeItem(deletionStorageKey(viewerId));
            } catch {
              /* Optional browser recovery. */
            }
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
  }, [client, request, revision, viewerId]);

  const submit = async () => {
    if (inFlight.current || request.accepted) return;
    inFlight.current = true;
    setStarted(true);
    setSending(true);
    setCommandError(undefined);
    persist(request);
    try {
      await requestDeletion(client, request);
      const accepted = { ...request, accepted: true };
      persist(accepted);
      setRequest(accepted);
      onAccepted?.(accepted);
      changed.current();
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
        description={`Delete "${name}"${target === 'Agent' ? ' and its Worktree' : ' and its contained runtime resources'}. This cannot be undone. Retained history follows the retention policy.`}
        acknowledgeLabel={`I understand this ${target} will be deleted.`}
        confirmLabel={`Delete ${target}`}
        busy={
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
      <p role="status">
        {observation?.complete
          ? `${target} deletion is confirmed.`
          : sending
            ? 'Submitting delete request.'
            : request.accepted
              ? 'Delete request accepted. Cleanup continues even if you close this page.'
              : 'The delete request has not been confirmed. Check the same request before trying another action.'}
      </p>
      <details>
        <summary>Cleanup progress</summary>
        {observation === undefined ? null : (
          <ProvisioningProgress title="Deletion progress" steps={observation.steps} />
        )}
        <LiveActivity
          entries={activityEntries(
            observation?.agentEvents ?? observation?.events ?? [],
            request.action.kind === 'delete_agent' ? 'agent' : 'sandbox',
          )}
          syncedAt={observation?.observedAt}
          stale={readError !== undefined}
          loading={observation === undefined && readError === undefined}
        />
      </details>
      <Presence>
        {observation?.failure === undefined ? null : (
          <p role="alert">
            Cleanup reported: {observation.failure.detail}. Deletion is not complete.
          </p>
        )}
      </Presence>
      <Presence>{commandError === undefined ? null : <p role="alert">{commandError}</p>}</Presence>
      <Presence>{readError === undefined ? null : <p role="alert">{readError}</p>}</Presence>
    </Modal>
  );
}
