export type Service = 'event-gateway' | 'image-manager';
export interface ServiceSession {
  authenticated: true;
  subject: string;
  permissions: readonly string[];
  expires_at: number;
}
/**
 * Reports a refused request. A 403 means the account is signed in but not
 * permitted; anything else ends the sign-in, so a caller passes the status.
 */
export type Denied = (status?: number) => void;
export class ServiceError extends Error {
  constructor(
    readonly status: number,
    message: string,
    readonly code?: string,
  ) {
    super(message);
  }
}
export async function serviceRequest<T>(
  service: Service,
  path: string,
  init: RequestInit = {},
): Promise<T> {
  const response = await fetch(`/api/${service}${path}`, {
    ...init,
    credentials: 'same-origin',
    cache: 'no-store',
    signal: init.signal ?? AbortSignal.timeout(15000),
    headers: {
      ...(init.body === undefined ? {} : { 'Content-Type': 'application/json' }),
      ...init.headers,
    },
  });
  if (!response.ok) {
    let detail = `Service request failed (${response.status}).`;
    let code: string | undefined;
    try {
      const body = (await response.json()) as { detail?: string; code?: string };
      if (typeof body.detail === 'string') detail = body.detail;
      if (typeof body.code === 'string') code = body.code;
    } catch {
      /* Status remains authoritative. */
    }
    throw new ServiceError(response.status, detail, code);
  }
  return response.status === 204 ? (undefined as T) : ((await response.json()) as T);
}
export function can(session: ServiceSession, service: Service, permission: string): boolean {
  const prefix = service === 'image-manager' ? 'cma-im' : 'cma-eg';
  return (
    session.permissions.includes(`${prefix}:admin`) ||
    session.permissions.includes(`${prefix}:${permission}`)
  );
}
export const segment = encodeURIComponent;
