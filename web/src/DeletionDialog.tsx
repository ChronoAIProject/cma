import { refreshSandboxCapacity } from './sandbox/useSandboxCapacity';
import { usePresenceExiting } from './ui/Presence';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { CmaWebClient } from './api/client';
import { detailReadSource } from './api/detailReads';
import { DeletionBlocked, stopDeletionBlock } from './agent/stopDeletion';
import { ApiError } from '@cma/client';
import {
  deletionTarget,
  observeDeletion,
  requestDeletion,
  type DeletionObservation,
  type DeletionRequest,
  type AgentDeletionPermissions,
} from './deletion';
import { forgetDeletion, persistDeletion } from './deletionRecovery';
import { RiskConfirmation } from './ui/RiskConfirmation';
import { Modal } from './ui/Modal';
import { Button } from './ui/Button';
import { DeletionImpactSummary } from './DeletionImpactSummary';
import { DeletionStatusPanel } from './DeletionStatusPanel';
import { transitionRefreshDelay } from './sandbox/lifecycle';
import { readableError } from './vocabulary/errors';

export function DeletionDialog({
  client,
  viewerId,
  initial,
  restored,
  onChanged,
  onAccepted,
  backgroundOnAcceptance = false,
  agentPermissions,
  enabled = true,
  onClose,
}: {
  readonly client: CmaWebClient;
  readonly viewerId: string;
  readonly initial: DeletionRequest;
  readonly restored: boolean;
  readonly onChanged: () => void;
  readonly onAccepted?: (request: DeletionRequest) => void;
  readonly backgroundOnAcceptance?: boolean;
  readonly agentPermissions?: AgentDeletionPermissions;
  readonly enabled?: boolean;
  readonly onClose: () => void;
}) {
  const exiting = usePresenceExiting();
  const [request, setRequest] = useState(initial);
  const [started, setStarted] = useState(restored);
  const [sendingScope, setSendingScope] = useState<object>();
  const latestRequest = useRef(initial);
  const [identity] = useState({ client, viewerId });
  const [observation, setObservation] = useState<DeletionObservation>();
  const [readError, setReadError] = useState<string>();
  const [commandError, setCommandError] = useState<string>();
  const [revision, setRevision] = useState(0);
  const inFlight = useRef(false);
  const backgrounded = useRef(false);
  const changed = useRef(onChanged);
  const completed = useRef(false);
  const autoDelete = useRef(false);
  const stopAllowed = agentPermissions?.stopAgent;
  const deleteAllowed = agentPermissions?.deleteAgent;
  const authority = useMemo(
    () =>
      stopAllowed === undefined || deleteAllowed === undefined
        ? undefined
        : {
            viewerId,
            permissions: { stopAgent: stopAllowed, deleteAgent: deleteAllowed },
          },
    [viewerId, stopAllowed, deleteAllowed],
  );
  const scope = useMemo(
    () => ({ client, viewerId, key: initial.key, enabled, exiting, authority }),
    [client, viewerId, initial.key, enabled, exiting, authority],
  );
  const sending = sendingScope === scope;
  const activeScope = useRef<{ token: object; controller: AbortController } | undefined>(undefined);
  useLayoutEffect(() => {
    const lifetime = { token: scope, controller: new AbortController() };
    activeScope.current = lifetime;
    autoDelete.current = false;
    inFlight.current = false;
    return () => {
      lifetime.controller.abort();
      activeScope.current = undefined;
    };
  }, [scope]);
  useEffect(() => {
    changed.current = onChanged;
  }, [onChanged]);
  const target = deletionTarget(request.action);
  const persist = useCallback(
    (value: DeletionRequest | undefined) => {
      if (value === undefined) forgetDeletion(identity.viewerId, request.key);
      else {
        latestRequest.current = value;
        persistDeletion(identity.viewerId, value);
        setRequest(value);
      }
    },
    [identity.viewerId, request.key],
  );

  const block = useCallback(
    (message: string) => {
      const value = latestRequest.current;
      if (!value.stop) return;
      autoDelete.current = false;
      if (value.stop && value.stop.phase !== 'blocked' && !value.accepted)
        persist({ ...value, stop: { ...value.stop, phase: 'blocked' } });
      setCommandError(message);
    },
    [persist],
  );

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
      void Promise.resolve()
        .then(() => {
          if (
            request.stop &&
            !request.accepted &&
            (identity.client !== client ||
              identity.viewerId !== viewerId ||
              !enabled ||
              !authority?.permissions.stopAgent ||
              !authority.permissions.deleteAgent)
          )
            throw new DeletionBlocked(
              'Your current access does not allow this Stop and delete request.',
            );
          return observeDeletion(
            client,
            request.action,
            controller.signal,
            request.accepted,
            events,
            agentEvents,
            authority && { ...authority, stopBeforeDelete: request.stop !== undefined },
          );
        })
        .then((next) => {
          if (controller.signal.aborted) return;
          const blocked = stopDeletionBlock(latestRequest.current, next);
          if (blocked) throw new DeletionBlocked(blocked);
          events = next.events;
          agentEvents = next.agentEvents;
          setObservation(next);
          setReadError(undefined);
          if (next.inProgress && !request.accepted) {
            const accepted = { ...request, accepted: true };
            persist(accepted);
            setStarted(true);
            changed.current();
          }
          if (next.complete) {
            completed.current = true;
            if (request.action.kind === 'delete_sandbox')
              void refreshSandboxCapacity(client, viewerId);
            forgetDeletion(identity.viewerId, request.key);
            changed.current();
          } else
            timer = setTimeout(poll, request.accepted ? transitionRefreshDelay(startedAt) : 1500);
        })
        .catch((error: unknown) => {
          if (controller.signal.aborted) return;
          if (
            error instanceof DeletionBlocked ||
            (error instanceof ApiError && [400, 401, 403, 404, 409, 422].includes(error.status))
          )
            block(error.message);
          if (error instanceof ApiError && [401, 403, 404].includes(error.status)) {
            events = [];
            agentEvents = [];
            setObservation(undefined);
          }
          // A missing resource can mean access loss, not proven deletion.
          setReadError(
            error instanceof DeletionBlocked
              ? undefined
              : error instanceof ApiError && [401, 403, 404].includes(error.status)
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
  }, [client, request, revision, viewerId, exiting, authority, identity, enabled, block, persist]);

  const submit = useCallback(async () => {
    const lifetime = activeScope.current;
    const blocked = () => latestRequest.current.stop?.phase === 'blocked';
    const current = () =>
      lifetime !== undefined &&
      activeScope.current === lifetime &&
      lifetime.token === scope &&
      !lifetime.controller.signal.aborted &&
      enabled &&
      !exiting;
    if (!current() || inFlight.current || request.accepted || request.stop?.phase === 'blocked')
      return;
    if (
      !started &&
      (observation === undefined || readError !== undefined || observation.blocked !== undefined)
    )
      return;
    if (request.stop && (!authority?.permissions.stopAgent || !authority.permissions.deleteAgent)) {
      block('Your current access does not allow this Stop and delete request.');
      return;
    }
    if (!request.stop && observation?.stopsAgent && (started || !authority)) {
      setCommandError('This delete request cannot continue until the Agent is stopped.');
      return;
    }
    inFlight.current = true;
    setStarted(true);
    setSendingScope(scope);
    setCommandError(undefined);
    const submitted: DeletionRequest =
      request.stop || !observation?.stopsAgent
        ? request
        : {
            ...request,
            stop: {
              key: crypto.randomUUID(),
              phase: 'requested',
              sandboxId: observation.sandboxId!,
              workspaceId: observation.agent!.workspaceId,
              worktreeId: observation.agent!.worktreeId,
            },
          };
    persist(submitted);
    try {
      if (submitted.action.kind === 'delete_agent' && (authority || submitted.stop)) {
        if (!authority || identity.client !== client || identity.viewerId !== viewerId)
          throw new DeletionBlocked('Current Agent access must be checked before continuing.');
        const fresh = await observeDeletion(
          detailReadSource(client),
          submitted.action,
          lifetime!.controller.signal,
          false,
          [],
          [],
          { ...authority, stopBeforeDelete: submitted.stop !== undefined },
        );
        if (!current() || blocked()) return;
        setObservation(fresh);
        const refusal = stopDeletionBlock(submitted, fresh) ?? fresh.blocked;
        if (refusal) throw new DeletionBlocked(refusal);
        if (!submitted.stop && fresh.stopsAgent)
          throw new Error(
            'The Agent is running again. Close this request and review Stop and delete.',
          );
        if (submitted.stop && fresh.agent?.status !== 'stopped') {
          autoDelete.current = true;
          if (submitted.stop.phase === 'requested') {
            await client.stopAgent(submitted.action.agentId, {
              idempotencyKey: submitted.stop.key,
            });
            if (!current() || blocked()) return;
            const stoppedRequest = {
              ...submitted,
              stop: { ...submitted.stop, phase: 'accepted' as const },
            };
            persist(stoppedRequest);
          }
          setRevision((value) => value + 1);
          return;
        }
      }
      if (!current() || blocked()) return;
      autoDelete.current = false;
      await requestDeletion(client, submitted);
      if (!current()) return;
      const accepted = { ...submitted, accepted: true };
      persist(accepted);
      backgrounded.current = backgroundOnAcceptance;
      changed.current();
      onAccepted?.(accepted);
    } catch (error: unknown) {
      if (!current()) return;
      setCommandError(
        error instanceof Error && error.name !== 'AbortError'
          ? error.message
          : readableError(error, 'The request could not be confirmed. Check its current status.'),
      );
      if (
        submitted.stop &&
        (error instanceof DeletionBlocked ||
          (error instanceof ApiError && [400, 401, 403, 404, 409, 422].includes(error.status)))
      )
        block(error.message);
      if (
        !submitted.stop &&
        error instanceof ApiError &&
        [400, 401, 403, 404, 422].includes(error.status)
      ) {
        persist(undefined);
        setStarted(false);
      }
    } finally {
      if (current()) {
        inFlight.current = false;
        setSendingScope(undefined);
      }
    }
  }, [
    scope,
    identity,
    viewerId,
    block,
    enabled,
    exiting,
    request,
    started,
    observation,
    readError,
    authority,
    persist,
    client,
    backgroundOnAcceptance,
    onAccepted,
  ]);
  useEffect(() => {
    if (
      autoDelete.current &&
      request.stop &&
      request.stop.phase !== 'blocked' &&
      !request.accepted &&
      !sending &&
      observation?.agent?.status === 'stopped' &&
      !observation.blocked &&
      readError === undefined
    )
      void submit();
  }, [request, sending, observation, readError, submit]);
  const close = () => {
    activeScope.current?.controller.abort();
    activeScope.current = undefined;
    autoDelete.current = false;
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
        confirmLabel={observation?.stopsAgent ? 'Stop and delete' : `Delete ${target}`}
        confirmDisabled={
          !enabled ||
          observation === undefined ||
          readError !== undefined ||
          observation.blocked !== undefined
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
              disabled={sending || request.stop?.phase === 'blocked' || !enabled}
              onClick={() => {
                void submit();
              }}
            >
              {request.stop?.phase === 'requested' ? 'Retry stop request' : 'Check request'}
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
