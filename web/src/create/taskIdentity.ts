let storageVerified = true;
const revocationPrefix = 'cma:resource-revocation:';

function revocationOwner(key: string): string | undefined {
  const tuple = key.indexOf('[', revocationPrefix.length);
  if (tuple < 0) return undefined;
  try {
    const scope: unknown = JSON.parse(key.slice(tuple));
    return Array.isArray(scope) && scope.length >= 2 && typeof scope[0] === 'string'
      ? scope[0]
      : undefined;
  } catch {
    return undefined;
  }
}

export function requireTaskStorage() {
  if (!storageVerified)
    throw new Error(
      'Task storage could not be isolated for this account. Sign in again after restoring session storage.',
    );
}

export function retainTaskViewer(owner?: string) {
  try {
    const marker = 'cma:task-viewer';
    const previous = sessionStorage.getItem(marker);
    const reset = !storageVerified || !owner || (previous !== null && previous !== owner);
    const keys = Array.from({ length: sessionStorage.length }, (_, index) =>
      sessionStorage.key(index),
    );
    for (const key of keys) {
      if (
        key !== null &&
        ((key.startsWith(revocationPrefix) && (reset || revocationOwner(key) !== owner)) ||
          (reset &&
            (key.startsWith('cma:task-draft:') ||
              key.startsWith('cma:agent-creation:') ||
              key.startsWith('cma:agent-recovery:'))))
      )
        sessionStorage.removeItem(key);
    }
    if (owner) sessionStorage.setItem(marker, owner);
    else sessionStorage.removeItem(marker);
    storageVerified = true;
  } catch {
    storageVerified = false;
  }
}
