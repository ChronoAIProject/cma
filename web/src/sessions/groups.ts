/**
 * The three states a session is in, as far as its owner is concerned.
 *
 * The rail groups by these rather than filtering. A filter answers "show me
 * the stopped ones" by hiding the rest; a group answers "which are running,
 * which are set aside, which are finished" without asking anything, which is
 * the question somebody actually opens the rail with.
 *
 * Derived from the lifecycle rather than stored beside it: the lifecycle is
 * what the control plane says, and a second field agreeing with it is a second
 * field that can stop agreeing with it.
 */
import type { Session, SessionLifecycle } from './types';

export type GroupId = 'active' | 'paused' | 'terminated';

/**
 * Which group a session belongs to.
 *
 * `rotating` is active, not paused. It is briefly not interactive — §5 — but
 * it is a session being carried onto a new runtime rather than one somebody
 * set aside, and putting it under Paused would say its owner did something.
 */
export function groupOf(lifecycle: SessionLifecycle): GroupId {
  switch (lifecycle) {
    case 'idle':
      return 'paused';
    case 'terminated':
      return 'terminated';
    case 'provisioning':
    case 'ready':
    case 'running':
    case 'rotating':
      return 'active';
    default: {
      const exhaustive: never = lifecycle;
      return exhaustive;
    }
  }
}

/**
 * The groups, in the order the rail draws them, and whether each starts open.
 *
 * Active is open and the rest are closed: a rail carrying forty finished
 * sessions buries the two somebody is working in, and the two they are working
 * in are why they opened it.
 */
export const GROUPS: readonly {
  readonly id: GroupId;
  readonly label: string;
  readonly openByDefault: boolean;
}[] = [
  { id: 'active', label: 'Active', openByDefault: true },
  { id: 'paused', label: 'Paused', openByDefault: false },
  { id: 'terminated', label: 'Terminated', openByDefault: false },
];

/**
 * Split sessions into the three groups, keeping the order they arrived in.
 *
 * Sorting is the caller's, applied before this: the order is one preference
 * across the whole rail rather than three that can disagree.
 */
export function groupSessions(
  sessions: readonly Session[],
): Readonly<Record<GroupId, readonly Session[]>> {
  const groups: Record<GroupId, Session[]> = { active: [], paused: [], terminated: [] };
  for (const session of sessions) {
    groups[groupOf(session.lifecycle)].push(session);
  }
  return groups;
}

/*
 * Which groups are closed, remembered across reloads.
 *
 * A collapsed group is a setting about how a list is drawn, which is what the
 * arrangement preference already is — same reasoning, same storage. Only the
 * closures are stored, so a group added later opens or closes by its own
 * default rather than by whatever a previous build happened to write.
 *
 * Web storage is untrusted input: another script on the origin, an older build
 * or a newer one somebody rolled back from may have written it. Anything this
 * build does not recognise is dropped rather than trusted.
 */
const KEY = 'chrono-bot:rail-collapsed';

function storage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    // Safari in private mode and embedded webviews throw on access rather than
    // being empty. A lost preference is acceptable; a crashed console is not.
    return null;
  }
}

function isGroupId(value: unknown): value is GroupId {
  return GROUPS.some((group) => group.id === value);
}

/** The groups that should start closed, from storage or from their defaults. */
export function loadCollapsed(): ReadonlySet<GroupId> {
  const fallback = new Set(GROUPS.filter((group) => !group.openByDefault).map((group) => group.id));
  const raw = storage()?.getItem(KEY);
  if (raw === null || raw === undefined) {
    return fallback;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return fallback;
  }
  if (!Array.isArray(parsed)) {
    return fallback;
  }
  return new Set(parsed.filter(isGroupId));
}

/** Remember which groups are closed. A failure to store is not a failure. */
export function storeCollapsed(collapsed: ReadonlySet<GroupId>): void {
  try {
    storage()?.setItem(KEY, JSON.stringify([...collapsed]));
  } catch {
    // Quota, or a storage that refuses writes. The rail still works.
  }
}
