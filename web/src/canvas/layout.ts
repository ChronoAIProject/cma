/**
 * Where nodes sit on the canvas, and which of them hold a terminal.
 *
 * Pure functions, deliberately: the arrangement and the attachment budget are
 * the two decisions most likely to be argued about later, and both are easier
 * to change with confidence when they are not entangled with the DOM.
 */

/**
 * How many sessions may hold a terminal at once.
 *
 * Every attached node costs an xterm instance — its own renderer, its own
 * capped scrollback buffer, its own ResizeObserver — plus, for a running
 * session, one WebSocket to the relay. Browsers allow a few hundred sockets per
 * host, so the socket count is not the binding limit; memory and paint time
 * are. Twelve is roughly where a mid-range laptop still repaints the whole
 * canvas within a frame, and twelve capped buffers stay well inside a tab's
 * budget.
 *
 * Past that, extra sessions still get a node — the canvas exists to show the
 * whole fleet — but the node renders unattached: frame, label, and a line
 * saying so, with no terminal and no connection. Focusing one attaches it and
 * evicts whichever attached node has gone longest without focus, which is why
 * the window is ordered by recency of focus rather than by list position.
 *
 * Re-attaching is cheap and lossless in both directions. A running session's
 * screen is held by the control plane and repainted on reattach, and a stopped
 * one is read from its stored transcript, so a re-attached node comes back with
 * its history rather than blank.
 */
export const MAX_LIVE_NODES = 12;

/**
 * Choose which sessions hold a terminal.
 *
 * `recency` is most-recently-focused first. Ids in it that are no longer
 * sessions are ignored, so a caller may keep a longer history than the session
 * list without filtering it first.
 */
export function selectLiveWindow(
  ids: readonly string[],
  recency: readonly string[],
  cap: number = MAX_LIVE_NODES,
): ReadonlySet<string> {
  // A cap below one would attach nothing at all, including the node someone is
  // typing into. Treat it as the caller having got the arithmetic wrong.
  const budget = Math.max(1, Math.floor(cap));
  const present = new Set(ids);
  const live = new Set<string>();

  for (const id of recency) {
    if (live.size >= budget) {
      break;
    }
    if (present.has(id)) {
      live.add(id);
    }
  }

  // Fill any remaining budget in list order, so a fresh canvas with no focus
  // history still attaches the sessions someone can actually see first.
  for (const id of ids) {
    if (live.size >= budget) {
      break;
    }
    live.add(id);
  }

  return live;
}

/**
 * Column count for `count` nodes.
 *
 * Square-ish rather than one long row: a row would make the canvas pan only
 * horizontally, and a single column only vertically. Both directions carrying
 * content is what makes panning worth the affordance.
 */
export function gridColumns(count: number): number {
  if (!Number.isFinite(count) || count <= 1) {
    return 1;
  }
  return Math.ceil(Math.sqrt(Math.floor(count)));
}

/** Keys the canvas answers to. Anything else is left to the browser. */
export type CanvasArrow = 'ArrowLeft' | 'ArrowRight' | 'ArrowUp' | 'ArrowDown' | 'Home' | 'End';

export function isCanvasArrow(key: string): key is CanvasArrow {
  return (
    key === 'ArrowLeft' ||
    key === 'ArrowRight' ||
    key === 'ArrowUp' ||
    key === 'ArrowDown' ||
    key === 'Home' ||
    key === 'End'
  );
}

/**
 * Index a key press moves focus to, clamped to the grid.
 *
 * Clamped rather than wrapping: a wrap turns "I am at the right-hand edge" into
 * a silent jump to the other side of the canvas, and on a surface that has been
 * panned, the node they land on may not even be in view.
 */
export function nextNodeIndex(
  current: number,
  key: CanvasArrow,
  count: number,
  columns: number,
): number {
  if (count <= 0) {
    return -1;
  }
  const stride = Math.max(1, Math.floor(columns));
  const from = current < 0 || current >= count ? 0 : current;

  switch (key) {
    case 'ArrowLeft':
      return Math.max(0, from - 1);
    case 'ArrowRight':
      return Math.min(count - 1, from + 1);
    case 'ArrowUp':
      return Math.max(0, from - stride);
    case 'ArrowDown':
      return Math.min(count - 1, from + stride);
    case 'Home':
      return 0;
    case 'End':
      return count - 1;
    default: {
      // Exhaustiveness: a new navigation key must be given a movement
      // deliberately rather than silently doing nothing.
      const exhaustive: never = key;
      return exhaustive;
    }
  }
}
