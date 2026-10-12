import { useRef, useState } from 'react';
import { Presence } from './ui/Presence';
import type { CmaWebClient } from './api/client';
import { CommandKeys } from './api/client';
import type { TreeAction } from './shell/Tree';
import { DeletionDialog } from './DeletionDialog';
import { Button } from './ui/Button';
import { useConnectionGate } from './connections/useConnectionGate';
import { restoreDeletion, type DeleteAction, type DeletionRequest } from './deletion';
import { downloadAgentHistory } from './agent/historyExport';

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
) {
  const [deletion, setDeletion] = useState<
    { request: DeletionRequest; restored: boolean } | undefined
  >(() => {
    const request = restoreDeletion(viewerId);
    return request === undefined ? undefined : { request, restored: true };
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
          const transcript = await client.readAgentTranscript(action.agentId);
          downloadAgentHistory(action.agentId, transcript);
          break;
        }
      }
      keys.current.settle(scope);
      onChanged();
    } catch (failure: unknown) {
      setError(failure instanceof Error ? failure.message : 'The operation did not complete.');
    } finally {
      inFlight.current = false;
      setPending(false);
    }
  };
  const select = (action: TreeAction) => {
    if (action.kind === 'new_agent' || action.kind === 'new_workspace') {
      onCreate?.(action);
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
    pending: pending || deletion !== undefined || connections.pending,
    error,
    notice:
      deletion === undefined && retained !== undefined ? (
        <Button onClick={() => setDeletion({ request: retained, restored: true })}>
          View deletion status
        </Button>
      ) : null,
    overlay: (
      <>
        {connections.dialog}
        <Presence>
          {deletion === undefined ? null : (
            <DeletionDialog
              key={deletion.request.key}
              client={client}
              viewerId={viewerId}
              initial={deletion.request}
              restored={deletion.restored}
              onChanged={onChanged}
              onClose={() => setDeletion(undefined)}
            />
          )}
        </Presence>
      </>
    ),
  };
}
