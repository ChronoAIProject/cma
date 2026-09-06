import { useRef, useState } from 'react';
import type { CmaWebClient } from './api/client';
import { CommandKeys } from './api/client';
import type { TreeAction } from './shell/Tree';
import { RiskConfirmation } from './ui/RiskConfirmation';

export type CreateAction =
  Extract<TreeAction, { kind: 'new_agent' | 'new_workspace' }> | { kind: 'new_agent' };
type LifecycleAction = Exclude<TreeAction, { kind: 'new_agent' | 'new_workspace' }>;

export function useResourceActions(
  client: CmaWebClient,
  onChanged: () => void,
  onCreate?: (action: CreateAction) => void,
) {
  const [confirmation, setConfirmation] = useState<LifecycleAction>();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();
  const keys = useRef(new CommandKeys());
  const inFlight = useRef(false);
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
      }
      keys.current.settle(scope);
      setConfirmation(undefined);
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
    } else if (['delete_sandbox', 'remove_workspace', 'delete_agent'].includes(action.kind)) {
      setError(undefined);
      setConfirmation(action);
    } else {
      void execute(action);
    }
  };
  const target =
    confirmation?.kind === 'delete_sandbox'
      ? 'Sandbox'
      : confirmation?.kind === 'remove_workspace'
        ? 'Workspace'
        : 'Agent';
  return {
    select,
    pending,
    error,
    overlay: (
      <RiskConfirmation
        open={confirmation !== undefined}
        title={`Delete ${target}`}
        description={`Delete this ${target} and its contained resources. This cannot be undone.`}
        acknowledgeLabel={`I understand this ${target} and its contained resources will be deleted.`}
        confirmLabel={`Delete ${target}`}
        busy={pending}
        failure={error}
        onCancel={() => setConfirmation(undefined)}
        onConfirm={() => {
          if (confirmation !== undefined) void execute(confirmation);
        }}
      />
    ),
  };
}
