import type { AgentDetail, SandboxDetail } from '../api/types';

/** The server proves admission; the browser also checks its exact observed lineage and owner. */
export function bootstrapInputAllowed(
  agent: AgentDetail,
  parent: SandboxDetail,
  owner: string | undefined,
  operationId: string,
): boolean {
  const bootstrap = agent.bootstrap;
  return (
    owner !== undefined &&
    parent.owner === owner &&
    agent.status === 'starting' &&
    bootstrap !== undefined &&
    operationId !== '' &&
    bootstrap.operationId === operationId &&
    bootstrap.sandboxId === parent.id &&
    bootstrap.sandboxId === agent.sandbox.id &&
    bootstrap.workspaceId === agent.workspace.id &&
    agent.worktree.agentId === agent.id &&
    agent.worktree.workspaceId === agent.workspace.id &&
    agent.worktree.sandboxId === parent.id &&
    (parent.status === 'provisioning' || parent.status === 'installing') &&
    parent.lifecycle !== undefined &&
    parent.lifecycle.operation === undefined &&
    parent.lifecycle.checkpoint === undefined &&
    parent.lifecycle.pause.reason !== 'checkpoint_in_progress'
  );
}
