import type { ActivityEntry } from './types';

/**
 * Making the record readable, per docs/Web_Style.md §15.
 *
 * The activity log is the one place this product shows many rows at once, so
 * the two rules that make it legible — newest first, and narrow it down — live
 * here rather than inside the component that draws it.
 */

/** How a value looked in the record, and how much it stands out. */
export type Tone = 'live' | 'wait' | 'fail' | 'ended';

/**
 * How much attention an outcome deserves.
 *
 * Matching the core's own judgement: being turned away and failing are the two
 * worth noticing. A request that was simply unusable is the caller's mistake
 * and is not an incident, so it stays quiet.
 */
export function outcomeTone(outcome: string): Tone {
  switch (outcome) {
    case 'succeeded':
      return 'live';
    case 'denied':
      return 'fail';
    case 'failed':
      return 'wait';
    default:
      // Including `rejected`, and including anything a newer control plane has
      // started recording: an unfamiliar word is drawn plainly rather than
      // guessed at.
      return 'ended';
  }
}

/**
 * An action as a person reads it.
 *
 * The core's names are snake_case identifiers, which are exactly right on the
 * wire and hard to scan in a column. Nothing is translated — `session_stop`
 * becomes `session stop`, not "Someone stopped a session" — because a made-up
 * sentence would drift from what was actually recorded.
 */
export function readableAction(action: string): string {
  return action.replaceAll('_', ' ');
}

/**
 * Newest first, always.
 *
 * The control plane already orders the record this way; doing it again here
 * costs nothing and means the rule holds whichever implementation of the port
 * is behind it, including a fake in a test. Entries whose timestamp could not
 * be read go last: they cannot be placed in time, and putting them at the top
 * would push down the rows somebody opened the log to see.
 */
export function newestFirst(entries: readonly ActivityEntry[]): readonly ActivityEntry[] {
  return [...entries].sort((a, b) => (b.at ?? -Infinity) - (a.at ?? -Infinity));
}

/** What the activity view is currently narrowed to. */
export interface ActivityFilter {
  /** One subject, or every subject when absent. */
  readonly actor?: string;
  /** One action, or every action when absent. */
  readonly action?: string;
}

/**
 * The entries a filter leaves.
 *
 * Only the action is applied here. Narrowing by person is asked of the control
 * plane instead, because the record is served with a limit: filtering a page of
 * a hundred entries down to one person's three would show three, where asking
 * for that person's entries shows a hundred of theirs.
 */
export function filtered(
  entries: readonly ActivityEntry[],
  filter: ActivityFilter,
): readonly ActivityEntry[] {
  return filter.action === undefined
    ? entries
    : entries.filter((entry) => entry.action === filter.action);
}

/** Every action present in these entries, in the order a select should list them. */
export function actionsIn(entries: readonly ActivityEntry[]): readonly string[] {
  return [...new Set(entries.map((entry) => entry.action))].sort((a, b) => a.localeCompare(b));
}

/** Every subject these entries are attributed to. */
export function actorsIn(entries: readonly ActivityEntry[]): readonly string[] {
  const actors = entries
    .map((entry) => entry.actor)
    .filter((actor): actor is string => actor !== undefined);
  return [...new Set(actors)];
}
