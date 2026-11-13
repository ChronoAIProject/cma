import type { DeletionRequest } from './deletion';

export function deletionStorageKey(viewerId: string): string {
  return `cma:deletion:${viewerId}`;
}

export function restoreDeletion(viewerId: string): DeletionRequest | undefined {
  try {
    const value = JSON.parse(
      localStorage.getItem(deletionStorageKey(viewerId)) ?? 'null',
    ) as DeletionRequest | null;
    if (
      value === null ||
      typeof value.key !== 'string' ||
      value.key.length > 128 ||
      typeof value.accepted !== 'boolean'
    )
      return undefined;
    const action = value.action;
    const id =
      action?.kind === 'delete_sandbox'
        ? action.sandboxId
        : action?.kind === 'remove_workspace'
          ? action.workspaceId
          : action?.kind === 'delete_agent'
            ? action.agentId
            : undefined;
    return typeof id === 'string' && /^[a-zA-Z0-9_-]{1,128}$/.test(id) ? value : undefined;
  } catch {
    return undefined;
  }
}

