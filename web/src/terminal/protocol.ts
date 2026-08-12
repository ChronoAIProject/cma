/**
 * Wire protocol for the terminal attachment.
 *
 * Kept free of any DOM or emulator dependency so the rules that actually
 * matter — resize before the first data frame, opcode-based dispatch, the
 * closed set of control messages — can be tested without a browser.
 *
 * Framing is bimodal with no escaping: binary frames are terminal bytes, text
 * frames are JSON control messages. Never infer the kind from the payload.
 */

/** Control messages this client sends. */
export type ClientControl =
  | { t: 'resize'; cols: number; rows: number }
  | { t: 'signal'; sig: 'INT' | 'TERM' | 'QUIT' | 'HUP' }
  | { t: 'ping' };

/** Limits the server advertises when the attachment is ready. */
export interface ServerLimits {
  scrollbackLines: number;
  maxFrameBytes: number;
}

/** Control messages the server sends. */
export type ServerControl =
  | {
      t: 'ready';
      cols: number;
      rows: number;
      snapshot: boolean;
      conformsTo: string[];
      limits: ServerLimits;
    }
  | { t: 'exit'; code: number; reason: string }
  | { t: 'error'; code: string; detail: string }
  | { t: 'pong' };

/**
 * Parse a text frame into a server control message.
 *
 * Returns null rather than throwing: a frame this client does not understand
 * is ignorable, and tearing down a working terminal over an unknown control
 * message would be worse than skipping it.
 */
export function parseServerControl(raw: string): ServerControl | null {
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof value !== 'object' || value === null) return null;
  const t = (value as { t?: unknown }).t;
  if (t !== 'ready' && t !== 'exit' && t !== 'error' && t !== 'pong') return null;
  return value as ServerControl;
}

/** Build the resize frame that must precede any terminal byte. */
export function resizeFrame(cols: number, rows: number): string {
  return JSON.stringify({ t: 'resize', cols, rows } satisfies ClientControl);
}

/** Build a signal frame. */
export function signalFrame(sig: 'INT' | 'TERM' | 'QUIT' | 'HUP'): string {
  return JSON.stringify({ t: 'signal', sig } satisfies ClientControl);
}

/** Build a keepalive frame. */
export function pingFrame(): string {
  return JSON.stringify({ t: 'ping' } satisfies ClientControl);
}

/**
 * Whether a geometry is safe to send.
 *
 * A zero dimension is a normal transient during layout — a hidden container, a
 * tab being switched — and the server rejects it. Filtering here keeps a
 * routine layout state from producing an error round trip.
 */
export function isSendableGeometry(cols: number, rows: number): boolean {
  return Number.isFinite(cols) && Number.isFinite(rows) && cols > 0 && rows > 0;
}

/** Build the WebSocket URL for a session's terminal. */
export function terminalUrl(apiBaseUrl: string, sessionId: string): string {
  const base = apiBaseUrl.replace(/\/+$/, '');
  const wsBase = base.startsWith('https://')
    ? `wss://${base.slice('https://'.length)}`
    : base.startsWith('http://')
      ? `ws://${base.slice('http://'.length)}`
      : base;
  return `${wsBase}/api/v1/sessions/${encodeURIComponent(sessionId)}/terminal`;
}
