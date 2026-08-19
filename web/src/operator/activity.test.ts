import { describe, expect, it } from 'vitest';
import {
  actionsIn,
  actorsIn,
  filtered,
  newestFirst,
  outcomeState,
  readableAction,
} from './activity';
import type { ActivityEntry } from './types';

/**
 * Making the record readable.
 *
 * Order and narrowing are the two things that decide whether a page of a
 * hundred rows is usable, so they are pinned here rather than left to whatever
 * the server happened to send.
 */

function entry(overrides: Partial<ActivityEntry> & { id: string }): ActivityEntry {
  return {
    action: 'session_list',
    outcome: 'succeeded',
    detail: [],
    ...overrides,
  };
}

describe('order', () => {
  it('puts the newest first whatever order it arrived in', () => {
    const ordered = newestFirst([
      entry({ id: 'old', at: 100 }),
      entry({ id: 'new', at: 300 }),
      entry({ id: 'middle', at: 200 }),
    ]);
    expect(ordered.map((e) => e.id)).toEqual(['new', 'middle', 'old']);
  });

  it('puts entries that cannot be placed in time last', () => {
    // They cannot be ordered, and putting them at the top would push down the
    // rows somebody opened the log to read.
    const ordered = newestFirst([entry({ id: 'undated' }), entry({ id: 'dated', at: 1 })]);
    expect(ordered.map((e) => e.id)).toEqual(['dated', 'undated']);
  });

  it('leaves the caller’s own list alone', () => {
    const entries = [entry({ id: 'a', at: 1 }), entry({ id: 'b', at: 2 })];
    newestFirst(entries);
    expect(entries.map((e) => e.id)).toEqual(['a', 'b']);
  });
});

describe('narrowing', () => {
  const entries = [
    entry({ id: 'a', action: 'session_create', actor: 'ada' }),
    entry({ id: 'b', action: 'session_stop', actor: 'grace' }),
    entry({ id: 'c', action: 'session_create', actor: 'ada' }),
    entry({ id: 'd', action: 'session_create' }),
  ];

  it('keeps everything when nothing is chosen', () => {
    expect(filtered(entries, {})).toHaveLength(4);
  });

  it('narrows to one action', () => {
    expect(filtered(entries, { action: 'session_stop' }).map((e) => e.id)).toEqual(['b']);
  });

  it('leaves narrowing by person to the control plane', () => {
    // The record is served with a limit, so the server is the only place a
    // person filter can produce a full page of that person's entries.
    expect(filtered(entries, { actor: 'ada' })).toHaveLength(4);
  });

  it('lists the actions present, in a stable order', () => {
    expect(actionsIn(entries)).toEqual(['session_create', 'session_stop']);
  });

  it('lists the people present, without the unattributed ones', () => {
    expect(actorsIn(entries)).toEqual(['ada', 'grace']);
  });
});

describe('how an entry reads', () => {
  it('renders an action without inventing a sentence for it', () => {
    // A made-up sentence would drift from what was actually recorded.
    expect(readableAction('session_terminate')).toBe('session terminate');
  });

  it('marks being turned away and failing as the outcomes worth noticing', () => {
    // Being refused is a decision and failing is a fault, so they read
    // differently — the colours they carried are unchanged by the rename.
    expect(outcomeState('denied')).toBe('error');
    expect(outcomeState('failed')).toBe('warning');
    expect(outcomeState('succeeded')).toBe('done');
  });

  it('draws an unfamiliar outcome plainly rather than guessing at it', () => {
    // No dot at all. Every state the dot has asserts something specific, and
    // an outcome this build has never seen supports none of those claims.
    expect(outcomeState('rejected')).toBeUndefined();
    expect(outcomeState('something_new')).toBeUndefined();
  });
});
