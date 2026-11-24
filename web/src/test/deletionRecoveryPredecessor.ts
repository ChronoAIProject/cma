// Retained reader from 38c0de54, before Stop-and-delete receipts.
import type { DeletionRequest } from '../deletion';

export function deletionStorageKey(viewerId: string): string {
  return `cma:deletion:${viewerId}`;
}

function requestPrefix(viewerId: string): string {
  return `cma:deletion-requests:${encodeURIComponent(viewerId)}:`;
}

function decode(raw: string | null): DeletionRequest | undefined {
  try {
    const value = JSON.parse(raw ?? 'null') as DeletionRequest | null;
    if (
      value === null ||
      typeof value.key !== 'string' ||
      value.key.length === 0 ||
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

export function restoreDeletions(viewerId: string): readonly DeletionRequest[] {
  try {
    const requests = new Map<string, DeletionRequest>();
    const legacy = decode(localStorage.getItem(deletionStorageKey(viewerId)));
    if (legacy) requests.set(legacy.key, legacy);
    const prefix = requestPrefix(viewerId);
    for (let index = 0; index < localStorage.length; index++) {
      const key = localStorage.key(index);
      if (!key?.startsWith(prefix)) continue;
      const request = decode(localStorage.getItem(key));
      if (request && key === `${prefix}${encodeURIComponent(request.key)}`)
        requests.set(request.key, request);
    }
    return [...requests.values()];
  } catch {
    return [];
  }
}

export function restoreDeletion(viewerId: string): DeletionRequest | undefined {
  const requests = restoreDeletions(viewerId);
  return requests.find((request) => !request.accepted) ?? requests.at(-1);
}

// Independent keys prevent a second request or another tab from overwriting an
// accepted or uncertain deletion. The old single-request key remains readable.
export function persistDeletion(viewerId: string, request: DeletionRequest): void {
  try {
    localStorage.setItem(
      `${requestPrefix(viewerId)}${encodeURIComponent(request.key)}`,
      JSON.stringify(request),
    );
    const legacy = decode(localStorage.getItem(deletionStorageKey(viewerId)));
    if (!legacy || legacy.key === request.key)
      localStorage.setItem(deletionStorageKey(viewerId), JSON.stringify(request));
  } catch {
    // Browser recovery is optional; the mounted dialog retains the original key.
  }
}

export function forgetDeletion(viewerId: string, key: string): void {
  try {
    localStorage.removeItem(`${requestPrefix(viewerId)}${encodeURIComponent(key)}`);
    if (decode(localStorage.getItem(deletionStorageKey(viewerId)))?.key === key)
      localStorage.removeItem(deletionStorageKey(viewerId));
  } catch {
    // An accepted request can be observed again safely if storage is unavailable.
  }
}
