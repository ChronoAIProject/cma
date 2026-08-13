import type { TerminalDriver, TerminalSink } from '../terminal/types';

/**
 * Wraps a driver so that keystrokes only reach it while the gate is open.
 *
 * Canvas mode puts many terminals on screen at once, and the single property
 * that must never break is that a keystroke reaches the session someone is
 * looking at and no other. Three mechanisms enforce it, deliberately layered:
 *
 *   1. This gate, at the driver seam. It is the invariant: whatever xterm, the
 *      DOM or a future driver does, no byte leaves an unfocused node.
 *   2. `disableStdin` on every terminal but the focused one, so xterm never
 *      raises the data event in the first place.
 *   3. The DOM, which can only deliver a key event to one element at a time.
 *
 * The first is the one that is provable in a test, which is why it exists even
 * though 2 and 3 would usually be enough. A gate that never fires is cheap; a
 * keystroke delivered to the wrong agent session is not recoverable.
 *
 * The gate is also where docs/Web_Style.md §14 lands on the canvas: a session
 * that is provisioning, rotating, idle or terminated is never typed into, so
 * its gate is simply never opened, whether or not its node has focus.
 *
 * The gate owns its open/closed state rather than reading the caller's, so the
 * wrapper's identity stays stable across focus changes. That matters: the
 * terminal surface rebuilds its xterm instance whenever the driver identity
 * changes, which would clear the scrollback and re-attach the transport every
 * time focus moved.
 */

export interface InputGate {
  /** Hand this to the terminal surface in place of the real driver. */
  readonly driver: TerminalDriver;
  /** Open or close the keyboard path. Safe to call with the current state. */
  setOpen(open: boolean): void;
}

/** Starts closed: a node has to be told it has focus before it can be typed at. */
export function createInputGate(driver: TerminalDriver): InputGate {
  let open = false;

  return {
    setOpen(next: boolean): void {
      open = next;
    },

    driver: {
      attach(sink: TerminalSink): void {
        driver.attach(sink);
      },

      send(data: string): void {
        if (!open) {
          // Dropped, not queued. Replaying it when the node regains focus would
          // deliver input typed at a different terminal, minutes later and out
          // of context — the exact failure this gate exists to prevent, and the
          // same reasoning §14 gives for a read-only terminal.
          return;
        }
        driver.send(data);
      },

      resize(cols: number, rows: number): void {
        // Geometry is not input. An unfocused node still renders live output
        // and must keep its remote geometry correct, or its next visible frame
        // wraps against a stale width.
        driver.resize(cols, rows);
      },

      detach(): void {
        driver.detach();
      },
    },
  };
}
