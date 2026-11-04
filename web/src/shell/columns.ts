/**
 * The three columns, and what gives way — `docs/Web_Style.md` §9.
 *
 * A pure solver: the widths are a function of the viewport and the two
 * preferences, and nothing else. No hysteresis and no memory, which is what
 * makes recovery automatic — widening the window restores exactly what
 * narrowing it took away, because narrowing never wrote anything down.
 *
 * The chain is fixed and the order matters:
 *
 * 1. everything fits at its preferred width;
 * 2. the details column shrinks toward its minimum;
 * 3. the details column closes, and the centre absorbs whatever is left.
 *
 * **The sidebar never concedes.** It is the way to every other resource, so a
 * layout that took it away to make room would be one where the fix for "this
 * is too narrow" is unreachable.
 *
 * A closed details column is a *derived* zero rather than a preference of
 * zero: the preference is never rewritten, so a window that widens again
 * reopens it at the width its owner chose.
 */

export interface Columns {
  readonly sidebar: number;
  readonly center: number;
  readonly details: number;
}

/** The centre's floor. Only the final fallback may go under it. */
export const CENTER_MIN = 640;

export const SIDEBAR_MIN = 264;
export const SIDEBAR_MAX = 420;
export const SIDEBAR_DEFAULT = 344;

/** The closed sidebar: a 24px icon column between 16px paddings. */
export const SIDEBAR_COLLAPSED = 56;

/**
 * Below this the sidebar collapses on its own.
 *
 * Consumed by the frame rather than by the solver, which stays free of
 * breakpoints: the frame decides what the sidebar preference *is* at this
 * width, and the solver only resolves it.
 */
export const SIDEBAR_AUTO_COLLAPSE = 1024;

export const DETAILS_MIN = 300;
export const DETAILS_MAX = 520;
export const DETAILS_DEFAULT = 360;

export function computeWorkspaceColumns(
  viewport: number,
  sidebar: number,
  details?: number,
): Columns {
  const sidebarSize =
    sidebar === 0 ? SIDEBAR_COLLAPSED : clampWidth(sidebar, SIDEBAR_MIN, SIDEBAR_MAX);
  const available = Math.max(0, viewport - sidebarSize);
  if (available < 720) return { sidebar: sidebarSize, center: available, details: 0 };
  const panelSize = clampWidth(details ?? available / 2, 360, available - 360);
  return { sidebar: sidebarSize, center: available - panelSize, details: panelSize };
}

/** Hold a dragged width inside its range. */
export function clampWidth(px: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, Math.round(px)));
}

/**
 * Resolve the three widths for one frame.
 *
 * Preferences are re-clamped here rather than trusted: they cross a storage
 * boundary, and a stale one written by an older build would otherwise reach
 * the layout as a width it cannot draw.
 */
export function computeColumns(viewport: number, sidebar: number, details: number): Columns {
  const s = sidebar === 0 ? SIDEBAR_COLLAPSED : clampWidth(sidebar, SIDEBAR_MIN, SIDEBAR_MAX);
  const preferred = details === 0 ? 0 : clampWidth(details, DETAILS_MIN, DETAILS_MAX);

  if (s + preferred + CENTER_MIN <= viewport) {
    return { sidebar: s, center: viewport - s - preferred, details: preferred };
  }

  const shrunk = preferred === 0 ? 0 : Math.max(DETAILS_MIN, viewport - s - CENTER_MIN);
  if (s + shrunk + CENTER_MIN <= viewport) {
    return { sidebar: s, center: CENTER_MIN, details: shrunk };
  }

  return { sidebar: s, center: Math.max(0, viewport - s), details: 0 };
}
