/**
 * When a session was created, as its owner reads it.
 *
 * Fixed to Singapore time rather than the viewer's own. A session is a shared
 * object — someone may be told about it, or read it in a log — and a timestamp
 * that means a different moment depending on who is looking is worse than one
 * that is consistently in a stated zone.
 */

/** The zone every session time is shown in. */
export const DISPLAY_ZONE = 'Asia/Singapore';

/** Short label for the zone, so a reader knows what they are looking at. */
export const DISPLAY_ZONE_LABEL = 'SGT';

const TIME = new Intl.DateTimeFormat('en-GB', {
  timeZone: DISPLAY_ZONE,
  hour: '2-digit',
  minute: '2-digit',
  hour12: false,
});

const DATE = new Intl.DateTimeFormat('en-GB', {
  timeZone: DISPLAY_ZONE,
  day: '2-digit',
  month: 'short',
});

const FULL = new Intl.DateTimeFormat('en-GB', {
  timeZone: DISPLAY_ZONE,
  dateStyle: 'medium',
  timeStyle: 'medium',
  hour12: false,
});

/**
 * The label a tab carries.
 *
 * Time alone for a session created today, since that is what distinguishes
 * one from another within a working day; the date is added once it is no
 * longer today, when "14:32" stops being enough to place it.
 */
export function tabLabel(createdAt: number, now: number = Date.now()): string {
  const time = TIME.format(createdAt);
  return isSameDayInZone(createdAt, now) ? time : `${DATE.format(createdAt)} ${time}`;
}

/** The full timestamp, for a tooltip or a details panel. */
export function fullTimestamp(createdAt: number): string {
  return `${FULL.format(createdAt)} ${DISPLAY_ZONE_LABEL}`;
}

/** Whether two instants fall on the same day in the display zone. */
export function isSameDayInZone(a: number, b: number): boolean {
  return DATE.format(a) === DATE.format(b) && yearIn(a) === yearIn(b);
}

function yearIn(at: number): string {
  return new Intl.DateTimeFormat('en-GB', {
    timeZone: DISPLAY_ZONE,
    year: 'numeric',
  }).format(at);
}

/** Which way the session list is ordered. */
export type SortOrder = 'newest' | 'oldest';

/**
 * Order sessions for display.
 *
 * Newest first by default: the session someone wants is almost always the one
 * they just started, and making them scroll to it would be the wrong default
 * every time but the first.
 */
export function sortSessions<T extends { readonly createdAt: number }>(
  sessions: readonly T[],
  order: SortOrder,
): readonly T[] {
  // Copied before sorting: the input belongs to the caller, and sorting it in
  // place would mutate state React is holding.
  return [...sessions].sort((a, b) =>
    order === 'newest' ? b.createdAt - a.createdAt : a.createdAt - b.createdAt,
  );
}

/**
 * A full timestamp, or nothing at all.
 *
 * For values the control plane may not have yet. Rendering "Invalid Date" in
 * reference material is worse than rendering nothing.
 */
export function fullTimestampOrNone(at: number | undefined): string {
  return at === undefined || Number.isNaN(at) ? '—' : fullTimestamp(at);
}
