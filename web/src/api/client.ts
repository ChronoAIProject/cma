/**
 * The control plane's REST surface, as the browser sees it.
 *
 * Every request is same-origin and cookie-authenticated. The bundle is served
 * by a proxy that forwards /api/ and /auth/ to the control plane, so no host is
 * configured here and none should be: a build that knows the API's address
 * would need rebuilding to move it, and a cross-origin path would mean handling
 * a token in the browser, which the auth contract forbids.
 *
 * Responses are validated rather than trusted. This is a trust boundary, and a
 * malformed payload should surface as a clear failure rather than propagate as
 * undefined through the UI.
 */

import type { Kernel, Session, SessionLifecycle } from '../sessions/types';
import type { AgentOptions } from '../sessions/options';
import { isStage, type Stage } from '../sessions/progress';

/** A request that failed, carrying enough to tell the user why. */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
    this.name = 'ApiError';
  }

  /** Whether this means "not signed in" rather than "went wrong". */
  get isUnauthenticated(): boolean {
    return this.status === 401;
  }
}

const LIFECYCLES: readonly SessionLifecycle[] = [
  'provisioning',
  'ready',
  'running',
  'rotating',
  'idle',
  'terminated',
];

const KERNELS: readonly Kernel[] = ['codex', 'claude-code'];

/** Shape the control plane sends for a session. */
interface SessionPayload {
  id?: unknown;
  kernel?: unknown;
  status?: unknown;
  created_at?: unknown;
  stop_reason?: unknown;
}

/**
 * Convert a payload into the model the UI renders.
 *
 * Unknown lifecycles and kernels are rejected rather than coerced: showing a
 * session in a state the frontend does not model would misreport what the
 * control plane actually said.
 */
export function toSession(raw: unknown): Session {
  const p = raw as SessionPayload;
  if (typeof p?.id !== 'string' || p.id === '') {
    throw new ApiError(0, 'session payload carried no id');
  }
  if (typeof p.status !== 'string' || !LIFECYCLES.includes(p.status as SessionLifecycle)) {
    throw new ApiError(0, `session ${p.id} has an unrecognised status`);
  }
  if (typeof p.kernel !== 'string' || !KERNELS.includes(p.kernel as Kernel)) {
    throw new ApiError(0, `session ${p.id} names an unrecognised kernel`);
  }
  // The wire carries RFC 3339; the tab strip counts age in milliseconds. An
  // unparseable date falls back to now rather than rendering "NaN ago".
  const parsed = typeof p.created_at === 'string' ? Date.parse(p.created_at) : Number.NaN;
  return {
    id: p.id,
    kernel: p.kernel as Kernel,
    lifecycle: p.status as SessionLifecycle,
    createdAt: Number.isNaN(parsed) ? Date.now() : parsed,
    // Narrowed to the one distinction the interface acts on. Carrying the
    // control plane's full vocabulary here would invite the frontend to
    // reason about reasons it has no behaviour for.
    stopReason: p.stop_reason === 'error' ? 'error' : p.stop_reason ? 'other' : undefined,
  };
}

async function request(path: string, init?: RequestInit): Promise<unknown> {
  let response: Response;
  try {
    response = await fetch(path, {
      ...init,
      // The session cookie is the credential. Without this the browser omits
      // it and every call reads as anonymous.
      credentials: 'include',
      headers: { 'Content-Type': 'application/json', ...(init?.headers ?? {}) },
    });
  } catch (cause) {
    throw new ApiError(0, `cannot reach the control plane: ${String(cause)}`);
  }

  if (!response.ok) {
    // The control plane reports failures as a problem document; fall back to
    // the status when the body is not one.
    let detail = `request failed with ${response.status}`;
    try {
      const body = (await response.json()) as { detail?: unknown; title?: unknown };
      const text = body?.detail ?? body?.title;
      if (typeof text === 'string' && text !== '') {
        detail = text;
      }
    } catch {
      // A non-JSON error body is not itself an error worth reporting over the
      // status code that produced it.
    }
    throw new ApiError(response.status, detail);
  }

  if (response.status === 204) {
    return null;
  }
  return (await response.json()) as unknown;
}

/** Sessions belonging to the signed-in subject, newest first. */
export async function listSessions(): Promise<readonly Session[]> {
  const body = await request('/api/v1/sessions');
  const items = (body as { sessions?: unknown })?.sessions ?? body;
  if (!Array.isArray(items)) {
    throw new ApiError(0, 'session list was not a list');
  }
  return items.map(toSession);
}

/**
 * Provision a session on the given kernel.
 *
 * Options are sent as given. Omitting them is meaningful — the control plane
 * applies its own safe defaults — so an absent choice is never filled in here.
 */
export async function createSession(
  kernel: Kernel,
  cols: number,
  rows: number,
  options?: AgentOptions,
): Promise<Session> {
  const body = await request('/api/v1/sessions', {
    method: 'POST',
    body: JSON.stringify({ kernel, cols, rows, options }),
  });
  return toSession(body);
}

/**
 * End a session, leaving it listed.
 *
 * Distinct from removing it: a finished conversation is still worth reading,
 * and the control plane keeps it until its owner deletes it.
 */
export async function terminateSession(id: string): Promise<void> {
  await request(`/api/v1/sessions/${encodeURIComponent(id)}/terminate`, { method: 'POST' });
}

/**
 * Release a session's sandbox, keeping the session.
 *
 * The reversible half of ending something: nothing is running and nothing is
 * being paid for, while the history stays readable and the session can be
 * picked up again with `reviveSession`.
 */
export async function stopSession(id: string): Promise<void> {
  await request(`/api/v1/sessions/${encodeURIComponent(id)}/stop`, { method: 'POST' });
}

/** Give a stopped session a sandbox again, under the same id. */
export async function reviveSession(id: string): Promise<Session> {
  const body = await request(`/api/v1/sessions/${encodeURIComponent(id)}/revive`, {
    method: 'POST',
  });
  return toSession(body);
}

/**
 * Everything a session's terminal has shown.
 *
 * How a session with no sandbox is read. Bytes rather than text, because it is
 * an escape-sequence stream that only a terminal knows how to interpret.
 */
export async function fetchTranscript(id: string): Promise<Uint8Array> {
  const response = await fetch(`/api/v1/sessions/${encodeURIComponent(id)}/transcript`, {
    credentials: 'include',
  });
  if (!response.ok) {
    throw new ApiError(response.status, `could not read the session transcript`);
  }
  return new Uint8Array(await response.arrayBuffer());
}

/** Remove a session and its history permanently. */
export async function deleteSession(id: string): Promise<void> {
  await request(`/api/v1/sessions/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

/**
 * Follow a session's events until it stops provisioning.
 *
 * Creating a session returns before its runtime exists, so this is how the UI
 * learns what happened next. Resumes from a cursor and reconnects on its own,
 * which is why it is a stream of server-sent events rather than polling.
 *
 * Returns a cancel function; calling it closes the stream.
 */
export function followSession(
  id: string,
  onStatus: (lifecycle: SessionLifecycle, detail?: string) => void,
  onError: (message: string) => void,
  onStage?: (stage: Stage) => void,
): () => void {
  const source = new EventSource(`/api/v1/sessions/${encodeURIComponent(id)}/events`, {
    withCredentials: true,
  });

  source.onmessage = (message: MessageEvent) => {
    let payload: {
      event?: { type?: string; status?: string; detail?: string; stage?: string };
    };
    try {
      payload = JSON.parse(String(message.data)) as typeof payload;
    } catch {
      // A frame we cannot read is not worth tearing the stream down for; the
      // next one carries the same state.
      return;
    }
    // A step on the way to ready, reported so the interface can say which
    // part is taking the time rather than only that something is.
    const stage = payload.event?.stage;
    if (isStage(stage)) {
      onStage?.(stage);
    }

    const status = payload.event?.status;
    if (typeof status === 'string' && LIFECYCLES.includes(status as SessionLifecycle)) {
      // The detail explains a failure. Without it a session that could not
      // start is indistinguishable from one that ended normally.
      onStatus(status as SessionLifecycle, payload.event?.detail);
    }
  };

  source.onerror = () => {
    // EventSource reconnects by itself, so this is only worth reporting if the
    // browser has given up entirely.
    if (source.readyState === EventSource.CLOSED) {
      onError('lost contact with the control plane while starting the session');
    }
  };

  return () => source.close();
}

/** URL of the terminal relay for a session. */
export function terminalUrl(id: string): string {
  const scheme = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${scheme}//${window.location.host}/api/v1/sessions/${encodeURIComponent(id)}/terminal`;
}
