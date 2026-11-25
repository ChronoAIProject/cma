const key = 'cma.connection-same-tab.v1';

function localPath(path: unknown): path is string {
  if (typeof path !== 'string' || !/^\/launches\/[^/?#\\]+$/.test(path)) return false;
  const url = new URL(path, window.location.origin);
  return url.origin === window.location.origin && url.pathname + url.search === path;
}

export function rememberSameTabReturn(launchId: string, expiresAt: string): boolean {
  try {
    const path = `/launches/${encodeURIComponent(launchId)}`;
    const expires = Date.parse(expiresAt);
    if (!localPath(path) || !Number.isFinite(expires) || expires <= Date.now()) return false;
    sessionStorage.setItem(key, JSON.stringify({ path, expires }));
    return true;
  } catch {
    return false;
  }
}

export function readSameTabReturn(): string | undefined {
  if (import.meta.env.VITE_CONNECTION_INTENT_RECOVERY_ENABLED !== 'true') return undefined;
  try {
    const value: unknown = JSON.parse(sessionStorage.getItem(key) ?? 'null');
    if (typeof value !== 'object' || value === null) return undefined;
    const { path, expires } = value as { path?: unknown; expires?: unknown };
    if (
      localPath(path) &&
      typeof expires === 'number' &&
      Number.isFinite(expires) &&
      expires > Date.now()
    )
      return path;
  } catch {
    // A missing or invalid navigation hint cannot authorize a return.
  }
  return undefined;
}

export function clearSameTabReturn(): void {
  try {
    sessionStorage.removeItem(key);
  } catch {
    // The connection is verified independently after navigation.
  }
}
