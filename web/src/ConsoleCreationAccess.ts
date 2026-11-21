import type { User } from './auth/types';
import type { TreePermissions } from './shell/Tree';

export function consoleCreationAccess(
  user: User,
  navigationScope: string,
  scopeReady: boolean,
  accessUncertain: boolean,
  creationAvailable: boolean,
) {
  const canUse = !accessUncertain && user.capabilities.includes('use_own_sandboxes');
  const canStartNewAgent =
    !user.visitor && user.capabilities.includes('use_own_sandboxes') && creationAvailable;
  const canCreate = canStartNewAgent && canUse;
  const canCreateInScope = canStartNewAgent && navigationScope === '' && scopeReady;
  const newAgentUnavailable = !scopeReady
    ? 'Checking your access.'
    : navigationScope !== ''
      ? 'Switch to Personal to create an Agent.'
      : undefined;
  const permissions: TreePermissions = {
    createAgent: canCreate,
    createWorkspace: canCreate,
    pauseSandbox: canUse,
    removeWorkspace: canUse,
    stopAgent: canUse,
    deleteAgent: canUse,
    exportHistory: canUse,
    deleteSandbox: !accessUncertain && user.capabilities.includes('delete_own_sandbox'),
  };
  return { canStartNewAgent, canCreateInScope, newAgentUnavailable, permissions };
}
