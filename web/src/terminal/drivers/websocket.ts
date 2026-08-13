/**
 * The live terminal relay.
 *
 * Binary frames are terminal bytes in both directions; text frames are JSON
 * control messages. That split is the protocol's, not a convenience: it means
 * a keystroke can never be mistaken for a command, however it is encoded.
 *
 * The driver owns the socket and nothing else. Rendering, hardening and
 * geometry belong to the surface, so this file contains no escape handling —
 * bytes are passed through exactly as received.
 *
 * A dropped connection is recovered rather than reported. The control plane
 * keeps the screen for as long as the session runs and repaints it on
 * reattach, so reconnecting restores what was there; announcing every blip
 * would make a recoverable interruption look like lost work.
 */

import type { TerminalDriver, TerminalSink } from '../types';

/** Control frames the relay sends. */
interface ServerControl {
  t?: unknown;
  code?: unknown;
  detail?: unknown;
}

/**
 * Geometry declared when the surface has not measured itself yet.
 *
 * The relay will not proceed without a size, so it is better to open at a
 * conventional one and correct it a frame later than to stall the handshake.
 */
const FALLBACK_COLS = 80;
const FALLBACK_ROWS = 24;

/** Backoff between reattach attempts. */
const RECONNECT_BASE_MS = 500;
const RECONNECT_MAX_MS = 8000;

/**
 * How many times to reattach before giving up and saying so.
 *
 * Bounded, because retrying forever against a session that has genuinely ended
 * hides the ending behind an interface that merely looks slow.
 */
const RECONNECT_ATTEMPTS = 6;

export interface WebSocketDriverOptions {
  /** Absolute ws:// or wss:// URL of the session's terminal. */
  readonly url: string;
  /**
   * Opening text to submit once the relay is ready.
   *
   * Carried from the landing page so a visitor's first prompt is not lost to
   * the sign-in round trip.
   */
  readonly openingTurn?: string;
  /** Injected so tests can supply a double rather than a real socket. */
  readonly connect?: (url: string) => WebSocket;
  /** Injected so tests do not wait out the backoff. */
  readonly schedule?: (fn: () => void, ms: number) => void;
}

export function createWebSocketDriver(options: WebSocketDriverOptions): TerminalDriver {
  const open = options.connect ?? ((url: string) => new WebSocket(url));
  const later = options.schedule ?? ((fn: () => void, ms: number) => void setTimeout(fn, ms));

  let socket: WebSocket | null = null;
  let sink: TerminalSink | null = null;
  let detached = false;
  // Geometry is held until the socket can carry it. The relay requires it
  // before anything else, so it is sent the moment the connection opens.
  let pending: { cols: number; rows: number } | null = null;
  let opened = false;
  let ready = false;
  // Input typed before the relay is ready. Sending it early is a protocol
  // error, and dropping it would silently lose what someone typed.
  const queued: string[] = [];
  let attempts = 0;
  // The opening turn belongs to the first connection only. Replaying it on a
  // reconnect would resubmit a prompt nobody typed twice.
  let openingSent = false;

  function sendControl(message: object): void {
    if (socket !== null && socket.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify(message));
    }
  }

  function sendBytes(data: string): void {
    if (socket !== null && socket.readyState === WebSocket.OPEN) {
      socket.send(new TextEncoder().encode(data));
    }
  }

  /**
   * Declare geometry, which the relay requires before it will do anything else.
   *
   * Sent on open rather than on ready. The relay does not announce itself until
   * it has a size, so waiting for that announcement before sending one leaves
   * both sides waiting for the other — and any keystroke in the meantime is
   * rejected as arriving before the first resize.
   */
  function declareGeometry(): void {
    const size = pending ?? { cols: FALLBACK_COLS, rows: FALLBACK_ROWS };
    sendControl({ t: 'resize', cols: size.cols, rows: size.rows });
  }

  function handleControl(raw: string): void {
    let frame: ServerControl;
    try {
      frame = JSON.parse(raw) as ServerControl;
    } catch {
      sink?.fail('the terminal sent a control frame that could not be read');
      return;
    }

    switch (frame.t) {
      case 'ready': {
        ready = true;
        // A successful attach is what proves the connection is good, so the
        // backoff resets here rather than merely on open.
        attempts = 0;
        if (!openingSent && options.openingTurn !== undefined && options.openingTurn !== '') {
          openingSent = true;
          // Sent as ordinary input, newline included, so the agent receives it
          // exactly as though it had been typed.
          sendBytes(`${options.openingTurn}\n`);
        }
        // Anything typed while the relay was still coming up.
        while (queued.length > 0) {
          const next = queued.shift();
          if (next !== undefined) {
            sendBytes(next);
          }
        }
        break;
      }
      case 'pong':
        break;
      case 'error': {
        const detail = typeof frame.detail === 'string' ? frame.detail : '';
        const code = typeof frame.code === 'string' ? frame.code : 'error';
        sink?.fail(detail === '' ? `terminal error: ${code}` : `${code}: ${detail}`);
        break;
      }
      default:
        // An unknown control frame is ignored rather than surfaced. The
        // protocol may grow, and a new frame type is not a terminal failure.
        break;
    }
  }

  /** Try again, unless this has stopped looking like an interruption. */
  function scheduleReconnect(): void {
    if (detached) {
      return;
    }
    if (attempts >= RECONNECT_ATTEMPTS) {
      sink?.fail('lost the terminal connection and could not restore it');
      return;
    }
    const delay = Math.min(RECONNECT_BASE_MS * 2 ** attempts, RECONNECT_MAX_MS);
    attempts += 1;
    later(() => {
      if (!detached) {
        connect();
      }
    }, delay);
  }

  /** Open a connection and wire it to the sink. */
  function connect(): void {
    ready = false;
    opened = false;

    let ws: WebSocket;
    try {
      ws = open(options.url);
    } catch (cause) {
      sink?.fail(`cannot open the terminal: ${String(cause)}`);
      return;
    }
    socket = ws;
    ws.binaryType = 'arraybuffer';

    ws.onopen = () => {
      opened = true;
      declareGeometry();
    };

    ws.onmessage = (event: MessageEvent) => {
      if (typeof event.data === 'string') {
        handleControl(event.data);
        return;
      }
      // Terminal output. Decoded as UTF-8 with replacement, never validated:
      // the surface is responsible for what is safe to render.
      //
      // Both a buffer and a view over one are accepted. An `instanceof` check
      // against ArrayBuffer is not reliable here: a buffer created in another
      // realm fails it and would silently decode as empty, which reads as a
      // terminal producing nothing rather than as a bug.
      const data: unknown = event.data;
      let bytes: Uint8Array;
      if (ArrayBuffer.isView(data)) {
        const view = data as ArrayBufferView;
        bytes = new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
      } else if (data != null && typeof (data as ArrayBuffer).byteLength === 'number') {
        bytes = new Uint8Array(data as ArrayBuffer);
      } else {
        bytes = new Uint8Array();
      }
      sink?.write(new TextDecoder().decode(bytes));
    };

    ws.onerror = () => {
      // Deliberately silent: an error is always followed by a close, and the
      // close is what decides whether this is worth telling anyone about.
    };

    ws.onclose = (event: CloseEvent) => {
      if (detached) {
        return;
      }
      if (event.wasClean && event.code === 1000) {
        // The session itself ended. Reconnecting would reopen something that
        // is over.
        sink?.finish(event.reason === '' ? 'session ended' : event.reason);
        return;
      }
      // Anything else is an interruption — an idle proxy reclaiming the
      // socket, a network blip, a rollout. The screen is held server-side and
      // repainted on reattach, so this is recovered rather than announced.
      scheduleReconnect();
    };
  }

  return {
    attach(next: TerminalSink): void {
      sink = next;
      detached = false;
      attempts = 0;
      connect();
    },

    send(data: string): void {
      if (!ready) {
        // Held rather than sent: the relay rejects data before it is ready,
        // and losing a keystroke is worse than delivering it a moment late.
        queued.push(data);
        return;
      }
      sendBytes(data);
    },

    resize(cols: number, rows: number): void {
      pending = { cols, rows };
      if (opened) {
        sendControl({ t: 'resize', cols, rows });
      }
    },

    detach(): void {
      // Marked before closing so the close handler stays silent and schedules
      // no reconnect: a detach is the app's own doing.
      detached = true;
      ready = false;
      opened = false;
      queued.length = 0;
      sink = null;
      if (socket !== null) {
        const ws = socket;
        socket = null;
        ws.onopen = null;
        ws.onmessage = null;
        ws.onerror = null;
        ws.onclose = null;
        if (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING) {
          ws.close();
        }
      }
    },
  };
}
