import { describe, expect, it } from 'vitest';
import { byRecency, labelFor, namesFrom, nameOf } from './directory';
import type { Person } from './types';

/**
 * Naming people.
 *
 * The rule under test is the one from Web_Style.md §15: a subject identifier is
 * never drawn where a name goes. What makes that hold is that the answer is a
 * union — a caller cannot fall back to the identifier without saying so.
 */

const ADA: Person = {
  subject: '76fe9d91',
  displayName: 'Ada Lovelace',
  email: 'ada@example.test',
  lastSeen: Date.parse('2026-08-14T11:00:00Z'),
};

describe('what somebody is called', () => {
  it('prefers the account name', () => {
    expect(nameOf(ADA)).toBe('Ada Lovelace');
  });

  it('falls back to the address', () => {
    expect(nameOf({ subject: 'x', email: 'grace@example.test' })).toBe('grace@example.test');
  });

  it('treats a blank claim as no claim', () => {
    // Otherwise the row renders an empty label and looks broken.
    expect(nameOf({ subject: 'x', displayName: '   ', email: '  ' })).toBeUndefined();
  });

  it('has no name for somebody the provider released nothing about', () => {
    expect(nameOf({ subject: 'x' })).toBeUndefined();
  });
});

describe('how a subject is drawn', () => {
  it('draws somebody the directory knows by name', () => {
    const label = labelFor('76fe9d91', namesFrom([ADA]));
    expect(label).toEqual({ kind: 'named', name: 'Ada Lovelace', subject: '76fe9d91' });
  });

  it('draws an identifier as an identifier when nothing is known', () => {
    // Not as a name. A reader has to be able to tell which one they are
    // looking at, which is why this is a separate kind rather than a string.
    expect(labelFor('76fe9d91')).toEqual({ kind: 'identifier', subject: '76fe9d91' });
  });

  it('says so when there was no identity at all', () => {
    expect(labelFor(undefined)).toEqual({ kind: 'anonymous' });
    expect(labelFor('  ')).toEqual({ kind: 'anonymous' });
  });

  it('prefers a name the payload carried over the directory', () => {
    // The control plane resolved it as it read the row, so it cannot be
    // staler than a directory fetched separately.
    const label = labelFor('76fe9d91', namesFrom([ADA]), 'Ada L.');
    expect(label).toEqual({ kind: 'named', name: 'Ada L.', subject: '76fe9d91' });
  });

  it('ignores a blank name the payload carried', () => {
    expect(labelFor('76fe9d91', namesFrom([ADA]), '  ')).toEqual({
      kind: 'named',
      name: 'Ada Lovelace',
      subject: '76fe9d91',
    });
  });
});

describe('the order people are listed in', () => {
  it('puts whoever was here most recently first', () => {
    const older: Person = { subject: 'older', lastSeen: Date.parse('2026-03-01T00:00:00Z') };
    const never: Person = { subject: 'never' };
    expect(byRecency([never, older, ADA]).map((p) => p.subject)).toEqual([
      '76fe9d91',
      'older',
      'never',
    ]);
  });

  it('leaves the caller’s own list alone', () => {
    const people = [ADA, { subject: 'other', lastSeen: 0 }];
    byRecency(people);
    expect(people[0]).toBe(ADA);
  });
});
