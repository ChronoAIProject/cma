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
 */

import type { TerminalDriver, TerminalSink } from '../types';

/** Control frames the relay sends. */
interface ServerControl {
  t?: unknown;
  code?: unknown;
  detail?: unknown;
}

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
}

export function createWebSocketDriver(options: WebSocketDriverOptions): TerminalDriver {
  const open = options.connect ?? ((url: string) => new WebSocket(url));

  let socket: WebSocket | null = null;
  let sink: TerminalSink | null = null;
  let detached = false;
  // Geometry can be set before the socket opens. It is held and sent on ready
  // rather than dropped, so the first frame is painted at the right size.
  let pending: { cols: number; rows: number } | null = null;
  let ready = false;

  function sendControl(message: object): void {
    if (socket !== null && socket.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify(message));
    }
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
        if (pending !== null) {
          sendControl({ t: 'resize', cols: pending.cols, rows: pending.rows });
        }
        if (options.openingTurn !== undefined && options.openingTurn !== '') {
          // Sent as ordinary input, newline included, so the agent receives it
          // exactly as though it had been typed.
          socket?.send(new TextEncoder().encode(`${options.openingTurn}\n`));
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

  return {
    attach(next: TerminalSink): void {
      sink = next;
      detached = false;
      ready = false;

      let ws: WebSocket;
      try {
        ws = open(options.url);
      } catch (cause) {
        sink.fail(`cannot open the terminal: ${String(cause)}`);
        return;
      }
      socket = ws;
      ws.binaryType = 'arraybuffer';

      ws.onmessage = (event: MessageEvent) => {
        if (typeof event.data === 'string') {
          handleControl(event.data);
          return;
        }
        // Terminal output. Decoded as UTF-8 with replacement, never validated:
        // the surface is responsible for what is safe to render.
        //
        // Both a buffer and a view over one are accepted. An `instanceof`
        // check against ArrayBuffer is not reliable here: a buffer created in
        // another realm fails it and would silently decode as empty, which
        // reads as a terminal producing nothing rather than as a bug.
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
        // The event carries no useful detail by design, so this says what is
        // actionable rather than inventing a cause.
        sink?.fail('the terminal connection failed');
      };

      ws.onclose = (event: CloseEvent) => {
        if (detached) {
          return;
        }
        if (event.wasClean) {
          sink?.finish(event.reason === '' ? 'session ended' : event.reason);
        } else {
          sink?.fail('the terminal connection closed unexpectedly');
        }
      };
    },

    send(data: string): void {
      if (socket !== null && socket.readyState === WebSocket.OPEN) {
        socket.send(new TextEncoder().encode(data));
      }
    },

    resize(cols: number, rows: number): void {
      pending = { cols, rows };
      if (ready) {
        sendControl({ t: 'resize', cols, rows });
      }
    },

    detach(): void {
      // Marked before closing so the close handler stays silent: a detach is
      // not a failure the user needs to be told about.
      detached = true;
      ready = false;
      sink = null;
      if (socket !== null) {
        const ws = socket;
        socket = null;
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
