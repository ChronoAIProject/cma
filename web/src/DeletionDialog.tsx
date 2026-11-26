import { refreshSandboxCapacity } from './sandbox/useSandboxCapacity';
import { usePresenceExiting } from './ui/Presence';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { CmaWebClient } from './api/client';
import { detailReadSource } from './api/detailReads';
import { deletionWasSettled, type DeletionSettlements } from './useDeletionNoticeReconciliation';
import {
  DeletionBlocked,
  stopDeletionBlock,
  blockedBeforeDelete,
  deleteMayHaveBeenSent,
} from './agent/stopDeletion';
import { ApiError } from '@cma/client';
import {
  deletionTarget,
  observeDeletion,
  requestDeletion,
  type DeletionObservation,
  type DeletionRequest,
  type AgentDeletionPermissions,
} from './deletion';
import {
  forgetDeletion,
  persistDeletion,
  acknowledgeRetainedDeletion,
  sameDeletionRequest,
} from './deletionRecovery';
import { Modal } from './ui/Modal';
import { Button } from './ui/Button';
import { DeletionConfirmation } from './DeletionConfirmation';
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
  onAbandoned,
  onRequestChanged,
  backgroundOnAcceptance = false,
  agentPermissions,
  enabled = true,
  settlements,
  onClose,
}: {
  readonly client: CmaWebClient;
  readonly viewerId: string;
  readonly initial: DeletionRequest;
  readonly restored: boolean;
  readonly onChanged: () => void;
  readonly onAccepted?: (request: DeletionRequest) => void;
  readonly onAbandoned?: (request: DeletionRequest) => void;
  readonly onRequestChanged?: (request: DeletionRequest | undefined) => void;
  readonly backgroundOnAcceptance?: boolean;
  readonly agentPermissions?: AgentDeletionPermissions;
  readonly enabled?: boolean;
  readonly settlements?: DeletionSettlements;
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
  const requestChanged = useRef(onRequestChanged);
  const completed = useRef(false);
  const autoDelete = useRef(false);
  const blockedRevision = useRef(0);
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
  const complete =
    observation?.complete === true ||
    (enabled &&
      settlements?.client === identity.client &&
      client === identity.client &&
      settlements.viewerId === identity.viewerId &&
      viewerId === identity.viewerId &&
      settlements.requests.some((item) => sameDeletionRequest(item, request)));
  const sending = sendingScope === scope && !complete;
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
    requestChanged.current = onRequestChanged;
  }, [onChanged, onRequestChanged]);
  const target = deletionTarget(request.action);
  const persist = useCallback(
    (value: DeletionRequest | undefined) => {
      if (value === undefined) forgetDeletion(identity.viewerId, request.key);
      else {
        value = persistDeletion(
          identity.viewerId,
          latestRequest.current.accepted ? latestRequest.current : value,
        );
        latestRequest.current = value;
        setRequest(value);
      }
      requestChanged.current?.(value);
      return value;
    },
    [identity.viewerId, request.key],
  );
  const block = useCallback(
    (message: string) => {
      const value = latestRequest.current;
      if (!value.stop) return;
      blockedRevision.current += 1;
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
    let retirementFailed = false;
    const startedAt = Date.now();
    const settled = () => completed.current || deletionWasSettled(client, latestRequest.current);
    const poll = () => {
      if (
        reading ||
        retirementFailed ||
        controller.signal.aborted ||
        document.hidden ||
        settled() ||
        backgrounded.current
      )
        return;
      clearTimeout(timer);
      reading = true;
      void Promise.resolve()
        .then(() => {
          if (controller.signal.aborted || settled()) return;
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
          if (controller.signal.aborted || !next || (settled() && !next.complete)) return;
          const blocked = stopDeletionBlock(latestRequest.current, next);
          if (blocked) throw new DeletionBlocked(blocked);
          events = next.events;
          agentEvents = next.agentEvents;
          setObservation(next);
          retirementFailed = next.retirementFailed === true;
          setReadError(undefined);
          if ((next.inProgress || retirementFailed) && !request.accepted && !settled()) {
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
            requestChanged.current?.(undefined);
            changed.current();
          } else if (!retirementFailed)
            timer = setTimeout(poll, request.accepted ? transitionRefreshDelay(startedAt) : 1500);
        })
        .catch((error: unknown) => {
          if (controller.signal.aborted || settled()) return;
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
  const submit = useCallback(
    async (source: 'user' | 'observation' = 'user') => {
      const lifetime = activeScope.current;
      const beforeRead = blockedRevision.current;
      const blocked = () =>
        completed.current ||
        deletionWasSettled(client, latestRequest.current) ||
        blockedBeforeDelete(latestRequest.current) ||
        blockedRevision.current !== beforeRead;
      const current = () =>
        lifetime !== undefined &&
        activeScope.current === lifetime &&
        lifetime.token === scope &&
        !lifetime.controller.signal.aborted &&
        enabled &&
        !exiting;
      if (!current() || blocked() || inFlight.current || request.accepted) return;
      if (
        !started &&
        (observation === undefined || readError !== undefined || observation.blocked !== undefined)
      )
        return;
      if (
        request.stop &&
        (!authority?.permissions.stopAgent || !authority.permissions.deleteAgent)
      ) {
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
      if (source === 'user') setCommandError(undefined);
      let submitted: DeletionRequest =
        request.stop || !observation?.stopsAgent
          ? request
          : {
              ...request,
              deleteSent: false,
              stop: {
                key: crypto.randomUUID(),
                phase: 'requested',
                sandboxId: observation.sandboxId!,
                workspaceId: observation.agent!.workspaceId,
                worktreeId: observation.agent!.worktreeId,
              },
            };
      try {
        if (persist(submitted)?.accepted) return;
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
          if (!current() || blocked() || persist(submitted)?.accepted) return;
          setObservation(fresh);
          const refusal = stopDeletionBlock(submitted, fresh) ?? fresh.blocked;
          if (refusal) throw new DeletionBlocked(refusal);
          if (!submitted.stop && fresh.stopsAgent)
            throw new Error(
              'The Agent is running again. Close this request and review Stop and delete.',
            );
          if (submitted.stop && fresh.agent?.status !== 'stopped') {
            if (deleteMayHaveBeenSent(submitted))
              throw new DeletionBlocked(
                'The delete result is unconfirmed. Check the original request after stopping is confirmed.',
              );
            autoDelete.current = true;
            if (source === 'user' && submitted.stop.phase === 'requested') {
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
        if (!current() || blocked() || persist(submitted)?.accepted) return;
        autoDelete.current = false;
        if (submitted.stop) {
          submitted = { ...submitted, deleteSent: true };
          persist(submitted);
        }
        await requestDeletion(client, submitted);
        if (!current() || blocked()) {
          acknowledgeRetainedDeletion(identity.viewerId, submitted);
          return;
        }
        const accepted = { ...submitted, accepted: true };
        persist(accepted);
        backgrounded.current = backgroundOnAcceptance;
        changed.current();
        onAccepted?.(accepted);
      } catch (error: unknown) {
        if (!current() || blocked()) return;
        autoDelete.current = false;
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
    },
    [
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
    ],
  );
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
      void submit('observation');
  }, [request, sending, observation, readError, submit]);
  const invalidate = () => {
    activeScope.current?.controller.abort();
    activeScope.current = undefined;
    autoDelete.current = false;
  };
  const close = () => {
    invalidate();
    if (!started || completed.current) persist(undefined);
    onClose();
  };
  const abandon = () => {
    const value = latestRequest.current;
    if (value.stop?.phase !== 'blocked' || value.accepted) return;
    invalidate();
    if (!deleteMayHaveBeenSent(value)) {
      persist(undefined);
      onAbandoned?.(value);
    }
    onClose();
  };
  const name = observation?.name ?? target;
  if (!started && !complete)
    return (
      <DeletionConfirmation
        client={client}
        viewerId={viewerId}
        action={request.action}
        observation={observation}
        enabled={enabled}
        readError={readError}
        commandError={commandError}
        onCancel={close}
        onConfirm={() => void submit()}
      />
    );
  return (
    <Modal
      open
      title={
        complete
          ? `${target} deleted`
          : observation?.retirementFailed
            ? `${target} deletion stopped`
            : `Deleting ${target}`
      }
      description={name}
      dismissible={!sending}
      onClose={close}
      footer={
        <>
          <Button disabled={sending} onClick={close}>
            {complete ? 'Done' : 'Close'}
          </Button>
          {!request.accepted && !complete ? (
            <Button
              variant="primary"
              disabled={sending || blockedBeforeDelete(request) || !enabled}
              onClick={() => void submit()}
            >
              {request.stop?.phase === 'requested' && !deleteMayHaveBeenSent(request)
                ? 'Retry stop request'
                : 'Check request'}
            </Button>
          ) : null}
          {request.stop?.phase === 'blocked' && !request.accepted && !complete ? (
            <Button onClick={abandon}>End this request</Button>
          ) : null}
          {(readError !== undefined || observation?.retirementFailed) && !complete ? (
            <Button onClick={() => setRevision((value) => value + 1)}>Refresh status</Button>
          ) : null}
        </>
      }
    >
      <DeletionStatusPanel
        request={request}
        observation={observation}
        complete={complete}
        sending={sending}
        commandError={commandError}
        readError={readError}
      />
    </Modal>
  );
}
