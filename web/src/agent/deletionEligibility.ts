import type { AgentStatus, SandboxSummary } from '../api/types';
import { sandboxChildrenAvailable } from '../sandbox/lifecycle';

export interface AgentDeletionPermissions {
  readonly stopAgent: boolean;
  readonly deleteAgent: boolean;
}

export function agentDeletionReason(
  status: AgentStatus,
  permissions: AgentDeletionPermissions,
  sandbox: SandboxSummary,
  viewerId?: string,
  stopBeforeDelete = false,
): string | undefined {
  if (!permissions.deleteAgent) return 'Your account cannot delete this Agent.';
  if (!sandboxChildrenAvailable(sandbox))
    return 'Open the parent environment to review its availability before deleting this Agent.';
  if (sandbox.lifecycle?.checkpoint !== undefined) return 'Wait for the Sandbox to finish saving.';
  if (!stopBeforeDelete && (status === 'stopped' || status === 'failed')) return undefined;
  if (status === 'deleted') return 'This Agent has been deleted.';
  if (!permissions.stopAgent) return 'Your account cannot stop this Agent before deletion.';
  if (viewerId === undefined || sandbox.owner !== viewerId)
    return 'Current owner access must be checked before stopping and deleting this Agent.';
}
