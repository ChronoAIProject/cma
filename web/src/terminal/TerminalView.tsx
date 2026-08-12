import { useEffect, useRef, useState } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import {
  isSendableGeometry,
  parseServerControl,
  pingFrame,
  resizeFrame,
  terminalUrl,
} from './protocol';

/** How often to send a keepalive. Ingress idle timeouts sit well below a terminal's natural idle period. */
const KEEPALIVE_MS = 20_000;

interface Props {
  apiBaseUrl: string;
  sessionId: string;
  onStatus: (status: string) => void;
}

/**
 * A live terminal attached to one session.
 *
 * The byte stream originates from a process running the user's own code, so
 * the emulator is deliberately hardened: sequences that would reach out of the
 * viewport and act on the host are disabled rather than trusted.
 */
export function TerminalView({ apiBaseUrl, sessionId, onStatus }: Props) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const term = new Terminal({
      convertEol: false,
      cursorBlink: true,
      fontFamily:
        'ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace',
      fontSize: 13,
      scrollback: 2000,
      allowProposedApi: true,
      theme: { background: '#0b0f14', foreground: '#d7dde5', cursor: '#7aa2f7' },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);

    // --- hardening -------------------------------------------------------
    // OSC 52 lets terminal output silently overwrite the host clipboard.
    // Returning true consumes the sequence so nothing acts on it.
    term.parser.registerOscHandler(52, () => true);
    // Window manipulation (title, size, icon) must never be driven by the
    // session's output. Consume the sequences rather than honouring them.
    term.parser.registerCsiHandler({ final: 't' }, () => true);
    term.options.windowsMode = false;
    // ---------------------------------------------------------------------

    let socket: WebSocket | null = null;
    let keepalive: ReturnType<typeof setInterval> | null = null;
    let disposed = false;
    let ready = false;

    const safeFit = () => {
      try {
        fit.fit();
      } catch {
        /* the container is mid-layout; the next resize will settle it */
      }
    };

    const sendGeometry = () => {
      if (!socket || socket.readyState !== WebSocket.OPEN) return;
      const { cols, rows } = term;
      if (!isSendableGeometry(cols, rows)) return;
      socket.send(resizeFrame(cols, rows));
    };

    const connect = () => {
      const url = terminalUrl(apiBaseUrl, sessionId);
      socket = new WebSocket(url);
      // Terminal output is bytes, not text. Without this the browser hands
      // back Blobs and every frame needs an async read.
      socket.binaryType = 'arraybuffer';

      socket.onopen = () => {
        onStatus('connected');
        safeFit();
        // The resize must precede any data frame: a full-screen application
        // painted at the wrong size produces corruption that does not
        // self-heal until the next full repaint.
        sendGeometry();
        keepalive = setInterval(() => {
          if (socket?.readyState === WebSocket.OPEN) socket.send(pingFrame());
        }, KEEPALIVE_MS);
      };

      socket.onmessage = (event) => {
        // Dispatch on the frame type, never by inspecting the payload.
        if (typeof event.data === 'string') {
          const msg = parseServerControl(event.data);
          if (!msg) return;
          if (msg.t === 'ready') {
            ready = true;
            onStatus('ready');
          } else if (msg.t === 'error') {
            setError(`${msg.code}: ${msg.detail}`);
            onStatus('error');
          } else if (msg.t === 'exit') {
            onStatus(`exited (${msg.reason})`);
            term.writeln(`\r\n\x1b[33m[session ended: ${msg.reason}]\x1b[0m`);
          }
          return;
        }
        term.write(new Uint8Array(event.data as ArrayBuffer));
      };

      socket.onclose = () => {
        if (disposed) return;
        onStatus(ready ? 'disconnected' : 'closed before ready');
        if (keepalive) clearInterval(keepalive);
      };

      socket.onerror = () => {
        if (!disposed) onStatus('connection error');
      };
    };

    // Keystrokes travel as binary, exactly as typed. Sending them as text
    // would make every keypress arrive as a malformed control message.
    const dataSub = term.onData((data) => {
      if (socket?.readyState !== WebSocket.OPEN) return;
      socket.send(new TextEncoder().encode(data));
    });

    const observer = new ResizeObserver(() => {
      safeFit();
      sendGeometry();
    });
    observer.observe(host);

    connect();
    term.focus();

    return () => {
      disposed = true;
      if (keepalive) clearInterval(keepalive);
      observer.disconnect();
      dataSub.dispose();
      socket?.close();
      term.dispose();
    };
  }, [apiBaseUrl, sessionId, onStatus]);

  return (
    <div className="terminal-wrap">
      {error && <div className="terminal-error">{error}</div>}
      <div className="terminal-host" ref={hostRef} />
    </div>
  );
}
