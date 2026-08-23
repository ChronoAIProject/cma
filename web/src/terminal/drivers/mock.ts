import type { TerminalDriver, TerminalSink } from '../types';

/**
 * Stand-in session driver.
 *
 * The WebSocket terminal relay (#14) does not exist yet, so this produces a
 * plausible session locally. It exists to prove the seam and to make the
 * authenticated surface exercisable end to end; it is replaced wholesale by
 * the real relay without any component changing.
 *
 * It implements a local line editor because there is no pty echoing input
 * back. A real driver does none of this — the remote pty owns echo.
 */

export interface MockDriverOptions {
  readonly sessionId: string;
  readonly kernel: string;
  /** Prompt carried over from the landing page, replayed as the first turn. */
  readonly openingTurn?: string;
}

const PROMPT = '\x1b[1;35moperator\x1b[0m\x1b[36m@cma\x1b[0m\x1b[90m:~$\x1b[0m ';

export function createMockDriver(options: MockDriverOptions): TerminalDriver {
  let sink: TerminalSink | null = null;
  let line = '';
  let detached = false;
  const timers = new Set<number>();

  const later = (fn: () => void, delay: number) => {
    const id = window.setTimeout(() => {
      timers.delete(id);
      if (!detached) {
        fn();
      }
    }, delay);
    timers.add(id);
  };

  const respond = (turn: string) => {
    sink?.write(`\r\n\x1b[90m⠋ dispatching to ${options.kernel}…\x1b[0m\r\n`);
    later(() => {
      sink?.write(`\x1b[32m✓\x1b[0m received: ${turn}\r\n`);
      sink?.write(
        `\x1b[90mthe control plane has no runtime attached yet — this is a local stand-in.\x1b[0m\r\n`,
      );
      sink?.write(PROMPT);
    }, 420);
  };

  return {
    attach(next: TerminalSink): void {
      sink = next;
      detached = false;
      sink.write(`\x1b[90msession ${options.sessionId} · ${options.kernel}\x1b[0m\r\n\r\n`);

      if (options.openingTurn !== undefined && options.openingTurn.length > 0) {
        // The visitor's landing-page prompt. Echoed, then left for them to
        // send: auto-submitting text typed before an auth redirect would be an
        // irreversible action taken on their behalf with no chance to edit.
        sink.write(PROMPT);
        line = options.openingTurn;
        sink.write(options.openingTurn);
      } else {
        sink.write(PROMPT);
      }
    },

    send(data: string): void {
      if (sink === null) {
        return;
      }
      for (const char of data) {
        if (char === '\r') {
          const turn = line.trim();
          line = '';
          if (turn.length === 0) {
            sink.write(`\r\n${PROMPT}`);
          } else {
            respond(turn);
          }
        } else if (char === '\x7f' || char === '\b') {
          if (line.length > 0) {
            line = line.slice(0, -1);
            sink.write('\b \b');
          }
        } else if (char >= ' ') {
          line += char;
          sink.write(char);
        }
        // Other control bytes are dropped: without a pty there is nothing
        // meaningful to do with them, and echoing them would corrupt the line.
      }
    },

    resize(): void {
      // No remote geometry to inform. A real driver sends a resize frame here.
    },

    detach(): void {
      detached = true;
      for (const id of timers) {
        window.clearTimeout(id);
      }
      timers.clear();
      sink = null;
      line = '';
    },
  };
}
