interface ConnectionReturn {
  readonly handoff_id: string;
  readonly expires_at: string;
}

function key(request: string, service: string): string {
  return `cma:connection-return:${encodeURIComponent(request)}:${encodeURIComponent(service)}`;
}

export function readConnectionReturn(
  request: string,
  service: string,
): ConnectionReturn | undefined {
  try {
    const raw = sessionStorage.getItem(key(request, service));
    if (raw === null) return undefined;
    const value: unknown = JSON.parse(raw);
    if (
      typeof value !== 'object' ||
      value === null ||
      !('handoff_id' in value) ||
      typeof value.handoff_id !== 'string' ||
      !value.handoff_id ||
      !('expires_at' in value) ||
      typeof value.expires_at !== 'string' ||
      !(Date.parse(value.expires_at) > Date.now())
    ) {
      clearConnectionReturn(request, service);
      return undefined;
    }
    return { handoff_id: value.handoff_id, expires_at: value.expires_at };
  } catch {
    return undefined;
  }
}

export function rememberConnectionReturn(
  request: string,
  service: string,
  value: ConnectionReturn,
): void {
  try {
    sessionStorage.setItem(
      key(request, service),
      JSON.stringify({
        handoff_id: value.handoff_id,
        expires_at: value.expires_at,
      }),
    );
  } catch {
    // Account setup still works when browser storage is unavailable.
  }
}

export function clearConnectionReturn(request: string, service: string): void {
  try {
    sessionStorage.removeItem(key(request, service));
  } catch {
    // A later read remains owner-checked by the connection endpoint.
  }
}
