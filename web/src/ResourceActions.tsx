import { useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { ResourceChange } from './agent/resourceChange';
import { Presence } from './ui/Presence';
import type { CmaWebClient } from './api/client';
import { CommandKeys } from './api/client';
import type { TreeAction } from './shell/Tree';
import { StopAgentDialog } from './agent/StopAgentDialog';
import { DeletionDialog } from './DeletionDialog';
import { useConnectionGate } from './connections/useConnectionGate';
import {
  restoreDeletion,
  type DeleteAction,
  type DeletionRequest,
  type AgentDeletionPermissions,
} from './deletion';
import { restoreDeletions } from './deletionRecovery';
import {
  reconcileDeletionRequests,
  type DeletionSettlements,
} from './useDeletionNoticeReconciliation';
import { DeletionNotice, DeletionProgress } from './DeletionNotice';
import { AgentDeletionNotice } from './AgentDeletionNotice';
import { useDeletionNotice, type DeletionNoticeContext } from './useDeletionNotice';
import { useHistoryExport } from './agent/useHistoryExport';
import type { AppRoute } from './routing/routes';
import { SandboxActionStatus, type SandboxActionRequest } from './sandbox/SandboxActionStatus';

export type CreateAction =
  | Extract<TreeAction, { kind: 'new_agent' | 'new_workspace' }>
  | { kind: 'new_agent' }
  | { kind: 'new_sandbox' };
type LifecycleAction = Exclude<
  TreeAction,
  { kind: 'new_agent' | 'new_workspace' | 'export_history' | 'export_retained_conversation' }
>;

export function useResourceActions(
  client: CmaWebClient,
  onChanged: (target: ResourceChange) => void,
  onCreate?: (action: CreateAction) => void,
  viewerId = 'current',
  onNavigate?: (route: AppRoute) => void,
  enabled = true,
  agentPermissions?: AgentDeletionPermissions,
  settlements?: DeletionSettlements,
  noticeContext: DeletionNoticeContext = { view: '', environment: true },
) {
  // Commands may settle after navigation; target matching belongs to the current view.
  const changed = useRef(onChanged);
  useLayoutEffect(() => {
    changed.current = onChanged;
  }, [onChanged]);
  const [deletion, setDeletion] = useState<
    { request: DeletionRequest; restored: boolean; origin?: object } | undefined
  >(() => {
    const request = restoreDeletion(viewerId);
    return request === undefined || request.accepted ? undefined : { request, restored: true };
  });
  const [deletionRequests, setDeletionRequests] = useState(() => restoreDeletions(viewerId));
  const notice = useDeletionNotice(viewerId, deletionRequests, noticeContext);
  const settled =
    settlements?.client === client && settlements.viewerId === viewerId ? settlements.requests : [];
  const [deletionScope, setDeletionScope] = useState({ viewerId, settlements });
  if (deletionScope.viewerId !== viewerId || deletionScope.settlements !== settlements) {
    setDeletionScope({ viewerId, settlements });
    setDeletionRequests((current) =>
      reconcileDeletionRequests(
        deletionScope.viewerId === viewerId ? current : [],
        restoreDeletions(viewerId),
        settled,
      ),
    );
  }
  const refreshDeletions = () => {
    setDeletionRequests((current) =>
      reconcileDeletionRequests(current, restoreDeletions(viewerId), settled),
    );
  };
  const reopenDeletion = (request: DeletionRequest) =>
    setDeletion({
      request: restoreDeletions(viewerId).find((item) => item.key === request.key) ?? request,
      restored: true,
    });
  const [stopping, setStopping] = useState<{ agentId: string; key: string }>();
  const [sandboxRequests, setSandboxRequests] = useState<readonly SandboxActionRequest[]>([]);
  const setSandboxRequest = (request: SandboxActionRequest) =>
    setSandboxRequests((current) => [
      ...current.filter((item) => item.action.sandboxId !== request.action.sandboxId),
      request,
    ]);
  const deletingSandboxIds = new Set(
    deletionRequests.flatMap((request) =>
      request.accepted && request.action.kind === 'delete_sandbox'
        ? [request.action.sandboxId]
        : [],
    ),
  );
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();
  const keys = useRef(new CommandKeys());
  const inFlight = useRef(false);
  const connectionScope = useMemo(() => ({ client, viewerId }), [client, viewerId]);
  const connections = useConnectionGate(enabled, connectionScope);
  const history = useHistoryExport(client, viewerId, enabled);
  const execute = async (action: LifecycleAction) => {
    if (!enabled || inFlight.current) return;
    inFlight.current = true;
    setPending(true);
    setError(undefined);
    connections.clearError();
    const scope = JSON.stringify(action);
    const options = { idempotencyKey: keys.current.for(scope) };
    const sandboxAction =
      action.kind === 'pause_sandbox' || action.kind === 'revive_sandbox' ? action : undefined;
    if (sandboxAction)
      setSandboxRequest({
        action: sandboxAction,
        key: options.idempotencyKey,
        phase: 'submitting',
      });
    try {
      switch (action.kind) {
        case 'pause_sandbox':
          await client.pauseSandbox(action.sandboxId, options);
          break;
        case 'revive_sandbox':
          await client.reviveSandbox(action.sandboxId, options);
          break;
        case 'delete_sandbox':
          await client.deleteSandbox(action.sandboxId, options);
          break;
        case 'remove_workspace':
          await client.removeWorkspace(action.workspaceId, options);
          break;
        case 'stop_agent':
          await client.stopAgent(action.agentId, options);
          break;
        case 'delete_agent':
          await client.deleteAgent(action.agentId, options);
          break;
      }
      keys.current.settle(scope);
      if (sandboxAction)
        setSandboxRequest({
          action: sandboxAction,
          key: options.idempotencyKey,
          phase: 'accepted',
        });
      changed.current(action);
    } catch (failure: unknown) {
      if (sandboxAction)
        setSandboxRequest({
          action: sandboxAction,
          key: options.idempotencyKey,
          phase: 'uncertain',
        });
      setError(failure instanceof Error ? failure.message : 'The operation did not complete.');
    } finally {
      inFlight.current = false;
      setPending(false);
    }
  };
  const viewDeletion = (sandboxId: string) => {
    if (pending || stopping || deletion) return;
    const retained = deletionRequests.find(
      (request) =>
        request.action.kind === 'delete_sandbox' && request.action.sandboxId === sandboxId,
    );
    setDeletion({
      request: retained
        ? retained
        : {
            action: { kind: 'delete_sandbox', sandboxId },
            key: keys.current.for(JSON.stringify({ kind: 'delete_sandbox', sandboxId })),
            accepted: true,
          },
      restored: true,
    });
  };
  const select = (action: TreeAction, eligibility?: AbortSignal) => {
    if (!enabled) return;
    if (action.kind === 'new_agent' || action.kind === 'new_workspace') {
      onCreate?.(action);
    } else if (action.kind === 'stop_agent') {
      if (pending || stopping || deletion) return;
      setStopping({ agentId: action.agentId, key: keys.current.for(JSON.stringify(action)) });
    } else if (action.kind === 'export_history' || action.kind === 'export_retained_conversation') {
      if (pending || stopping || deletion || connections.pending) return;
      void history.run(action.agentId, action.kind === 'export_retained_conversation');
    } else if (['delete_sandbox', 'remove_workspace', 'delete_agent'].includes(action.kind)) {
      if (pending || deletion !== undefined) return;
      setError(undefined);
      const same = deletionRequests.find(
        (request) => JSON.stringify(request.action) === JSON.stringify(action),
      );
      if (same) {
        reopenDeletion(same);
        return;
      }
      setDeletion(
        retained === undefined || retained.accepted || retained.stop?.phase === 'blocked'
          ? {
              request: {
                action: action as DeleteAction,
                key: keys.current.for(JSON.stringify(action)),
                accepted: false,
              },
              restored: false,
              origin: notice.origin,
            }
          : { request: retained, restored: true },
      );
    } else if (action.kind === 'revive_sandbox') {
      if (eligibility?.aborted) return;
      setError(undefined);
      const cancel = connections.cancel;
      const release = () => eligibility?.removeEventListener('abort', cancel);
      eligibility?.addEventListener('abort', cancel, { once: true });
      void connections.checkAndRun(
        { type: 'sandbox', id: action.sandboxId },
        viewerId,
        () => {
          release();
          if (!eligibility?.aborted) void execute(action);
        },
        undefined,
        release,
      );
    } else {
      void execute(action);
    }
  };
  const retained = deletionRequests.find((request) => !request.accepted) ?? deletionRequests.at(-1);
  const noticeDisabled = !enabled || pending || deletion !== undefined || stopping !== undefined;
  const agentDeletion =
    enabled && noticeContext.agentId
      ? deletionRequests.find(
          (request) =>
            request.accepted &&
            request.action.kind === 'delete_agent' &&
            request.action.agentId === noticeContext.agentId,
        )
      : undefined;
  const summary = (
    <DeletionNotice
      requests={deletionRequests.filter((request) => request !== agentDeletion)}
      disabled={noticeDisabled}
      onViewStatus={notice.openList}
      onDismiss={notice.dismiss}
    />
  );
  return {
    select,
    viewDeletion,
    environmentNotice: notice.environment ? summary : null,
    deletingSandboxIds,
    unknownDeletingSandboxIds: new Set(
      deletionRequests.flatMap((request) =>
        !request.accepted && request.action.kind === 'delete_sandbox'
          ? [request.action.sandboxId]
          : [],
      ),
    ),
    busySandboxIds: new Set(
      sandboxRequests
        .filter((request) => request.phase === 'accepted' || request.phase === 'submitting')
        .map((request) => request.action.sandboxId),
    ),
    pending:
      pending ||
      history.pending ||
      deletion !== undefined ||
      stopping !== undefined ||
      connections.pending,
    error: history.error ?? error ?? connections.error,
    viewDeletionStatus: noticeDisabled
      ? undefined
      : agentDeletion
        ? () => reopenDeletion(agentDeletion)
        : retained === undefined
          ? undefined
          : notice.openList,
    notice: (
      <>
        {history.notice ? (
          <p role="status" className="console__navigation-status">
            {history.notice}
          </p>
        ) : null}
        {agentDeletion ? (
          <AgentDeletionNotice
            key={agentDeletion.key}
            client={client}
            request={agentDeletion}
            onViewStatus={() => reopenDeletion(agentDeletion)}
            disabled={noticeDisabled}
          />
        ) : null}
        {notice.central ? summary : null}
        {notice.open && !noticeDisabled && deletionRequests.length > 0 ? (
          <DeletionProgress
            requests={deletionRequests}
            onClose={notice.closeList}
            onViewStatus={(request) => {
              notice.closeList();
              reopenDeletion(request);
            }}
          />
        ) : null}
        {sandboxRequests.map((sandboxRequest) => (
          <SandboxActionStatus
            key={sandboxRequest.key}
            client={client}
            request={sandboxRequest}
            onSettled={(complete) => {
              setSandboxRequests((current) =>
                current.map((item) =>
                  item.key === sandboxRequest.key
                    ? { ...item, phase: complete ? 'complete' : 'failed' }
                    : item,
                ),
              );
              changed.current(sandboxRequest.action);
            }}
            onClose={() =>
              setSandboxRequests((current) =>
                current.flatMap((item) =>
                  item.key !== sandboxRequest.key
                    ? [item]
                    : item.phase === 'accepted'
                      ? [{ ...item, hidden: true }]
                      : [],
                ),
              )
            }
            onNavigate={onNavigate}
          />
        ))}
      </>
    ),
    overlay: (
      <>
        {connections.dialog}
        {stopping ? (
          <StopAgentDialog
            client={client}
            agentId={stopping.agentId}
            requestKey={stopping.key}
            onChanged={() => changed.current({ kind: 'stop_agent', agentId: stopping.agentId })}
            onStopped={() =>
              keys.current.settle(JSON.stringify({ kind: 'stop_agent', agentId: stopping.agentId }))
            }
            onClose={() => setStopping(undefined)}
          />
        ) : null}
        <Presence>
          {deletion === undefined ? null : (
            <DeletionDialog
              key={deletion.request.key}
              client={client}
              viewerId={viewerId}
              agentPermissions={agentPermissions}
              enabled={enabled}
              settlements={settlements}
              initial={deletion.request}
              restored={deletion.restored}
              backgroundOnAcceptance
              onChanged={() => {
                refreshDeletions();
                changed.current(deletion.request.action);
              }}
              onRequestChanged={(request) => {
                if (request && deletion.origin) notice.started(request, deletion.origin);
                setDeletionRequests((current) => [
                  ...current.filter((item) => item.key !== deletion.request.key),
                  ...(request ? [request] : []),
                ]);
              }}
              onAccepted={() => setDeletion(undefined)}
              onAbandoned={(request) => keys.current.settle(JSON.stringify(request.action))}
              onClose={() => {
                setDeletion(undefined);
                refreshDeletions();
              }}
            />
          )}
        </Presence>
      </>
    ),
  };
}
