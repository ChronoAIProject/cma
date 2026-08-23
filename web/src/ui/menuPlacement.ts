/**
 * Where a portalled menu goes — `docs/Web_Style.md` §7.
 *
 * Separate from the component because it is arithmetic with no React in it,
 * and because it is the part worth testing directly: every case here is a
 * menu that would otherwise open partly off-screen, which is not something a
 * render test notices.
 */

/** Which edge of the anchor the list hangs from. */
export type MenuSide = 'bottom' | 'top' | 'right';

/** Which end of the anchor the list lines up with. */
export type MenuAlign = 'start' | 'end';

/** The gap between anchor and list. */
const OFFSET = 4;

/** How close to a viewport edge the list may come before it is pushed back. */
const MARGIN = 12;

export interface Viewport {
  readonly width: number;
  readonly height: number;
}

export interface Size {
  readonly width: number;
  readonly height: number;
}

/**
 * Place a list against its anchor, then keep it on screen.
 *
 * Clamping happens after placement rather than instead of it: a menu near the
 * right edge should still hang from its anchor and merely be nudged back, not
 * jump to the other side of the control that opened it.
 *
 * A list whose size is not known yet is placed but not clamped — clamping a
 * zero-width box against the right edge would pin it to the margin and then
 * visibly jump once it measured.
 */
export interface Placement {
  readonly left: number;
  readonly top: number;
}

export function placeMenu(
  anchor: DOMRect,
  list: Size,
  viewport: Viewport,
  side: MenuSide,
  align: MenuAlign,
): Placement {
  let left: number;
  let top: number;

  if (side === 'right') {
    left = anchor.right + OFFSET;
    top = anchor.top;
  } else {
    left = align === 'start' ? anchor.left : anchor.right - list.width;
    top = side === 'bottom' ? anchor.bottom + OFFSET : anchor.top - list.height - OFFSET;
  }

  if (list.width > 0) {
    left = Math.min(Math.max(left, MARGIN), viewport.width - list.width - MARGIN);
  }
  if (list.height > 0) {
    top = Math.min(Math.max(top, MARGIN), viewport.height - list.height - MARGIN);
  }

  return { left, top };
}
