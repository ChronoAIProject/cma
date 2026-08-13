import type { Person } from './types';

/**
 * Naming people, per docs/Web_Style.md §15.
 *
 * A subject identifier is the right thing to key state on and the wrong thing
 * to put where a name goes: every row looks alike, and a reader cannot tell an
 * identifier from somebody who happens to be called that.
 *
 * So the answer is a union rather than a string. A caller has to acknowledge
 * all three cases — named, identified, or not signed in at all — which is what
 * stops "the subject identifier" quietly becoming the fallback everywhere.
 */

/** How a person should be drawn. */
export type PersonLabel =
  | { readonly kind: 'named'; readonly name: string; readonly subject: string }
  /** The directory knows nothing about them; the identifier is all there is. */
  | { readonly kind: 'identifier'; readonly subject: string }
  /** The action carried no established identity. */
  | { readonly kind: 'anonymous' };

/** What this person is called, if the provider released anything. */
export function nameOf(person: Person): string | undefined {
  const name = person.displayName?.trim();
  if (name !== undefined && name !== '') {
    return name;
  }
  const email = person.email?.trim();
  return email !== undefined && email !== '' ? email : undefined;
}

/** Subject to name, for every person the directory knows a name for. */
export function namesFrom(people: readonly Person[]): ReadonlyMap<string, string> {
  const names = new Map<string, string>();
  for (const person of people) {
    const name = nameOf(person);
    if (name !== undefined) {
      names.set(person.subject, name);
    }
  }
  return names;
}

/**
 * How to draw a subject, given whatever the directory knows.
 *
 * `names` may be empty — a caller entitled to read the record but not the
 * directory sees identifiers, and drawing them as identifiers is the honest
 * rendering of exactly that.
 */
export function labelFor(
  subject: string | undefined,
  names: ReadonlyMap<string, string> = new Map(),
  known?: string,
): PersonLabel {
  if (subject === undefined || subject.trim() === '') {
    return { kind: 'anonymous' };
  }
  // A name the payload carried wins over the directory: the control plane
  // resolved it at the same moment it read the row, so it cannot be staler.
  const carried = known?.trim();
  const name = carried !== undefined && carried !== '' ? carried : names.get(subject);
  return name === undefined ? { kind: 'identifier', subject } : { kind: 'named', name, subject };
}

/**
 * People, most recently seen first.
 *
 * "Who is using this" is a question about now. Somebody who signed in an hour
 * ago is more interesting than somebody who signed in once in March, and an
 * alphabetical list buries them among people who have stopped coming.
 */
export function byRecency(people: readonly Person[]): readonly Person[] {
  return [...people].sort((a, b) => (b.lastSeen ?? 0) - (a.lastSeen ?? 0));
}
