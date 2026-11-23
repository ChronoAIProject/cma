import type { DeletionRequest } from './deletion';

export function deletionStorageKey(viewerId: string): string {
  return `cma:deletion:${viewerId}`;
}

function requestPrefix(viewerId: string): string {
  return `cma:deletion-requests:${encodeURIComponent(viewerId)}:`;
}

function combinedPrefix(viewerId: string): string {
  return `cma:stop-delete:v1:${encodeURIComponent(viewerId)}:`;
}

function decode(raw: string | null, combined = false): DeletionRequest | undefined {
  try {
    const stored = JSON.parse(raw ?? 'null');
    if (combined && stored?.version !== 1) return undefined;
    const value = (combined ? stored.request : stored) as DeletionRequest | null;
    if (
      value == null ||
      typeof value.key !== 'string' ||
      value.key.length === 0 ||
      value.key.length > 128 ||
      typeof value.accepted !== 'boolean' ||
      (value.deleteSent !== undefined && typeof value.deleteSent !== 'boolean')
    )
      return undefined;
    const action = value.action;
    const stop = value.stop;
    if (combined !== (stop !== undefined)) return undefined;
    if (
      stop !== undefined &&
      (action?.kind !== 'delete_agent' ||
        stop === null ||
        typeof stop.key !== 'string' ||
        stop.key.length === 0 ||
        stop.key.length > 128 ||
        stop.key === value.key ||
        !['requested', 'accepted', 'blocked'].includes(stop.phase) ||
        ![stop.sandboxId, stop.workspaceId, stop.worktreeId].every(
          (id) => typeof id === 'string' && /^[a-zA-Z0-9_-]{1,128}$/.test(id),
        ))
    )
      return undefined;
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
    const combined = combinedPrefix(viewerId);
    for (let index = 0; index < localStorage.length; index++) {
      const key = localStorage.key(index);
      const namespace = key?.startsWith(prefix)
        ? prefix
        : key?.startsWith(combined)
          ? combined
          : undefined;
      if (key === null || namespace === undefined) continue;
      const request = decode(localStorage.getItem(key), namespace === combined);
      if (request && key === `${namespace}${encodeURIComponent(request.key)}`)
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

export function sameDeletionRequest(left: DeletionRequest, right: DeletionRequest): boolean {
  return (
    left.key === right.key &&
    JSON.stringify(left.action) === JSON.stringify(right.action) &&
    (left.stop === undefined) === (right.stop === undefined) &&
    (!right.stop ||
      (['key', 'sandboxId', 'workspaceId', 'worktreeId'] as const).every(
        (field) => left.stop?.[field] === right.stop?.[field],
      ))
  );
}

function matchingDeletion(
  viewerId: string,
  expected: DeletionRequest,
): DeletionRequest | undefined {
  return restoreDeletions(viewerId).find((retained) => sameDeletionRequest(retained, expected));
}

// Independent keys prevent a second request or another tab from overwriting an
// accepted or uncertain deletion. The old single-request key remains readable.
export function persistDeletion(viewerId: string, request: DeletionRequest): DeletionRequest {
  const retained = matchingDeletion(viewerId, request);
  if (retained?.accepted) request = retained;
  try {
    if (request.stop) {
      localStorage.setItem(
        `${combinedPrefix(viewerId)}${encodeURIComponent(request.key)}`,
        JSON.stringify({ version: 1, request }),
      );
      return request;
    }
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
  return request;
}

export function forgetDeletion(viewerId: string, key: string): void {
  try {
    localStorage.removeItem(`${requestPrefix(viewerId)}${encodeURIComponent(key)}`);
    localStorage.removeItem(`${combinedPrefix(viewerId)}${encodeURIComponent(key)}`);
    if (decode(localStorage.getItem(deletionStorageKey(viewerId)))?.key === key)
      localStorage.removeItem(deletionStorageKey(viewerId));
  } catch {
    // An accepted request can be observed again safely if storage is unavailable.
  }
}

export function acknowledgeRetainedDeletion(viewerId: string, sent: DeletionRequest): void {
  const retained = matchingDeletion(viewerId, sent);
  if (!retained?.stop || retained.deleteSent === false) return;
  persistDeletion(viewerId, { ...retained, accepted: true });
}
