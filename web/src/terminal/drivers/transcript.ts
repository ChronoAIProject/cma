import { fetchTranscript } from '../../api/client';
import type { TerminalDriver, TerminalSink } from '../types';

/**
 * Shows what a session has already shown, and accepts nothing.
 *
 * For sessions with no sandbox behind them — stopped, or ended. There is
 * nothing to connect to, but there is plenty to read, and the alternative is a
 * panel saying the session exists while refusing to show any of it.
 *
 * Keystrokes are dropped rather than buffered. Holding them until a revive
 * would mean typing into one session and having it arrive in another sandbox,
 * minutes later, out of context.
 */
export function createTranscriptDriver(id: string, note: string): TerminalDriver {
  let sink: TerminalSink | null = null;
  let cancelled = false;

  return {
    attach(next: TerminalSink) {
      sink = next;
      // Cleared as well as set, because the contract allows a driver to be
      // attached, detached and attached again — which the canvas does every
      // time a node is evicted from the attachment budget and later focused
      // back into it. Leaving it set would make the second attach fetch the
      // transcript and then discard it, so the node would come back blank.
      cancelled = false;
      void (async () => {
        try {
          const bytes = await fetchTranscript(id);
          if (cancelled || sink === null) {
            return;
          }
          // Latin-1 rather than UTF-8: the transcript is a byte stream that
          // xterm decodes itself, and decoding it here would mangle any escape
          // sequence that happens to look like a multi-byte character.
          let text = '';
          for (const byte of bytes) {
            text += String.fromCharCode(byte);
          }
          sink.write(text);
          sink.finish(note);
        } catch (cause) {
          if (!cancelled) {
            sink?.fail(`Could not read this session's history: ${String(cause)}`);
          }
        }
      })();
    },

    send() {
      // Deliberately nothing. A read-only session says so through the
      // interface around it, not by silently swallowing input into a buffer
      // that will never be delivered.
    },

    resize() {
      // The transcript was produced at the geometry the session had. Reflowing
      // it to a new one would rewrap output the agent had already laid out.
    },

    detach() {
      cancelled = true;
      sink = null;
    },
  };
}
