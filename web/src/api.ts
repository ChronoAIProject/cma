/**
 * Client for the control plane's REST surface.
 *
 * Every call carries credentials, because the browser authenticates with the
 * session cookie the API issues rather than a token this app holds. Errors are
 * surfaced with the server's own problem detail so a user sees why something
 * failed rather than a bare status code.
 */

/** Runtime configuration, delivered by the server rather than baked into the bundle. */
export interface RuntimeConfig {
  apiBaseUrl: string;
}

declare global {
  interface Window {
    __CHRONO_CONFIG__?: Partial<RuntimeConfig>;
  }
}

/**
 * Resolve the API origin.
 *
 * A bundle cannot know where its API lives — the same artifact is deployed to
 * several environments — so the server injects it at runtime and this falls
 * back to the page's own origin when it has not.
 */
export function resolveConfig(): RuntimeConfig {
  const injected = typeof window !== 'undefined' ? window.__CHRONO_CONFIG__ : undefined;
  const apiBaseUrl =
    injected?.apiBaseUrl && injected.apiBaseUrl.trim().length > 0
      ? injected.apiBaseUrl.trim()
      : typeof window !== 'undefined'
        ? window.location.origin
        : '';
  return { apiBaseUrl: apiBaseUrl.replace(/\/+$/, '') };
}

/** A session as the API renders it. */
export interface SessionView {
  id: string;
  status: string;
  recovery_state: string;
  stop_reason?: string;
  kernel: string;
  cols: number;
  rows: number;
  last_seq: number;
  created_at: string;
}

/** An error carrying the server's problem detail. */
export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

async function request<T>(baseUrl: string, path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, {
    ...init,
    credentials: 'include',
    headers: { 'Content-Type': 'application/json', ...(init?.headers ?? {}) },
  });
  if (!response.ok) {
    // Prefer the server's own explanation; fall back to the status only when
    // the body is not a problem document.
    let detail = `${response.status} ${response.statusText}`;
    try {
      const body = await response.json();
      if (body && typeof body === 'object') {
        detail = body.detail ?? body.title ?? detail;
      }
    } catch {
      /* body was not JSON; the status line stands */
    }
    throw new ApiError(response.status, detail);
  }
  if (response.status === 204) return undefined as T;
  return (await response.json()) as T;
}

/** Whether the caller is authenticated, and as whom. */
export async function fetchSession(
  baseUrl: string,
): Promise<{ authenticated: boolean; subject?: string }> {
  try {
    return await request(baseUrl, '/auth/session');
  } catch (e) {
    if (e instanceof ApiError && e.status === 401) return { authenticated: false };
    throw e;
  }
}

/** List the caller's sessions. */
export function listSessions(baseUrl: string): Promise<SessionView[]> {
  return request(baseUrl, '/api/v1/sessions');
}

/** Create a session bound to a kernel. */
export function createSession(
  baseUrl: string,
  kernel: string,
  cols: number,
  rows: number,
): Promise<SessionView> {
  return request(baseUrl, '/api/v1/sessions', {
    method: 'POST',
    body: JSON.stringify({ kernel, cols, rows }),
  });
}

/** End a session. */
export function terminateSession(baseUrl: string, id: string): Promise<void> {
  return request(baseUrl, `/api/v1/sessions/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

/** Kernels this deployment can back a session with. */
export function listKernels(baseUrl: string): Promise<{ kernels: string[] }> {
  return request(baseUrl, '/api/v1/kernels');
}
