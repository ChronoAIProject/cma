import { useRef, useState } from 'react';
import { Presence } from './ui/Presence';
import type { CmaWebClient } from './api/client';
import { CommandKeys } from './api/client';
import type { TreeAction } from './shell/Tree';
import { StopAgentDialog } from './agent/StopAgentDialog';
import { DeletionDialog } from './DeletionDialog';
import { Button } from './ui/Button';
import { useConnectionGate } from './connections/useConnectionGate';
import { restoreDeletion, type DeleteAction, type DeletionRequest } from './deletion';
import { downloadAgentHistory } from './agent/historyExport';
import type { AppRoute } from './routing/routes';
import { SandboxActionStatus, type SandboxActionRequest } from './sandbox/SandboxActionStatus';

export type CreateAction =
  | Extract<TreeAction, { kind: 'new_agent' | 'new_workspace' }>
  | { kind: 'new_agent' }
  | { kind: 'new_sandbox' };
type LifecycleAction = Exclude<TreeAction, { kind: 'new_agent' | 'new_workspace' }>;

export function useResourceActions(
  client: CmaWebClient,
  onChanged: () => void,
  onCreate?: (action: CreateAction) => void,
  viewerId = 'current',
  onNavigate?: (route: AppRoute) => void,
) {
  const [deletion, setDeletion] = useState<
    { request: DeletionRequest; restored: boolean } | undefined
  >(() => {
    const request = restoreDeletion(viewerId);
    return request === undefined ? undefined : { request, restored: true };
  });
  const [stopping, setStopping] = useState<{ agentId: string; key: string }>();
  const [sandboxRequest, setSandboxRequest] = useState<SandboxActionRequest>();
  const [deletingSandboxIds, setDeletingSandboxIds] = useState<ReadonlySet<string>>(() => {
    const request = restoreDeletion(viewerId);
    return new Set(
      request?.accepted && request.action.kind === 'delete_sandbox'
        ? [request.action.sandboxId]
        : [],
    );
  });
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();
  const keys = useRef(new CommandKeys());
  const inFlight = useRef(false);
  const connections = useConnectionGate();
  const execute = async (action: LifecycleAction) => {
    if (inFlight.current) return;
    inFlight.current = true;
    setPending(true);
    setError(undefined);
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
        case 'export_history': {
          const history = await client.exportAgentHistory(action.agentId);
          downloadAgentHistory(action.agentId, history);
          break;
        }
      }
      keys.current.settle(scope);
      if (sandboxAction)
        setSandboxRequest({
          action: sandboxAction,
          key: options.idempotencyKey,
          phase: 'accepted',
        });
      onChanged();
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
    const retained = restoreDeletion(viewerId);
    setDeletion({
      request:
        retained?.action.kind === 'delete_sandbox' && retained.action.sandboxId === sandboxId
          ? retained
          : {
              action: { kind: 'delete_sandbox', sandboxId },
              key: keys.current.for(JSON.stringify({ kind: 'delete_sandbox', sandboxId })),
              accepted: true,
            },
      restored: true,
    });
  };
  const select = (action: TreeAction) => {
    if (action.kind === 'new_agent' || action.kind === 'new_workspace') {
      onCreate?.(action);
    } else if (action.kind === 'stop_agent') {
      if (pending || stopping || deletion) return;
      setStopping({ agentId: action.agentId, key: keys.current.for(JSON.stringify(action)) });
    } else if (action.kind === 'export_history') {
      void execute(action);
    } else if (['delete_sandbox', 'remove_workspace', 'delete_agent'].includes(action.kind)) {
      if (pending || deletion !== undefined) return;
      setError(undefined);
      const retained = restoreDeletion(viewerId);
      setDeletion(
        retained === undefined
          ? {
              request: {
                action: action as DeleteAction,
                key: keys.current.for(JSON.stringify(action)),
                accepted: false,
              },
              restored: false,
            }
          : { request: retained, restored: true },
      );
    } else if (action.kind === 'revive_sandbox') {
      connections.run({ type: 'sandbox', id: action.sandboxId }, () => void execute(action));
    } else {
      void execute(action);
    }
  };
  const retained = restoreDeletion(viewerId);
  return {
    select,
    viewDeletion,
    deletingSandboxIds,
    pending:
      pending ||
      sandboxRequest?.phase === 'accepted' ||
      deletion !== undefined ||
      stopping !== undefined ||
      connections.pending,
    error,
    notice: (
      <>
        {sandboxRequest === undefined ? null : (
          <SandboxActionStatus
            key={sandboxRequest.key}
            client={client}
            request={sandboxRequest}
            onSettled={(complete) => {
              setSandboxRequest((current) =>
                current?.key === sandboxRequest.key
                  ? { ...current, phase: complete ? 'complete' : 'failed' }
                  : current,
              );
              onChanged();
            }}
            onClose={() => setSandboxRequest(undefined)}
            onNavigate={onNavigate}
          />
        )}
        {deletion === undefined && retained !== undefined ? (
          <Button onClick={() => setDeletion({ request: retained, restored: true })}>
            View deletion status
          </Button>
        ) : null}
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
            onChanged={onChanged}
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
              initial={deletion.request}
              restored={deletion.restored}
              onChanged={onChanged}
              onAccepted={(request) => {
                if (request.action.kind === 'delete_sandbox') {
                  const sandboxId = request.action.sandboxId;
                  setDeletingSandboxIds((current) => new Set([...current, sandboxId]));
                }
              }}
              onClose={() => setDeletion(undefined)}
            />
          )}
        </Presence>
      </>
    ),
  };
}
