import type { TreeAction } from './Tree';

export function selectAgentAction(
  id: string,
  agentId: string,
  onAction: ((action: TreeAction) => void) | undefined,
  parentSandboxId?: string,
) {
  if (id === 'delete_sandbox') {
    if (parentSandboxId !== undefined)
      onAction?.({ kind: 'delete_sandbox', sandboxId: parentSandboxId });
  } else if (
    id === 'stop_agent' ||
    id === 'delete_agent' ||
    id === 'export_history' ||
    id === 'export_retained_conversation'
  )
    onAction?.({ kind: id, agentId });
}
