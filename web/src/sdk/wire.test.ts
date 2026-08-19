import { describe, expect, it } from 'vitest';
import { PATHS, STATE_DELTA_PATCHES, frameKey } from './wire';
import { toEvent } from '../agui/events';
import { EMPTY, reduce } from '../chat/conversation';

/**
 * The wire, pinned.
 *
 * Both defects these cover were the client and the service each being
 * internally consistent while disagreeing with each other, and neither failed
 * loudly. A test that reads the client's own constant back to itself would
 * have passed throughout, so each of these asserts against a frame shaped the
 * way `docs/API_AGUI.md` says the service shapes it.
 */

describe('a STATE_DELTA carries its patches under the name the service uses', () => {
  it('reads them, rather than finding nothing', () => {
    // The defect: this client read `patch` while the service sends `delta`,
    // so every STATE_DELTA arrived carrying an empty list and applied to
    // nothing. No session ever reported what it had spent.
    expect(STATE_DELTA_PATCHES).toBe('delta');

    const event = toEvent({
      type: 'STATE_DELTA',
      threadId: 'sesn_1',
      runId: 'run_1',
      seq: 4,
      delta: [{ op: 'replace', path: PATHS.inputTokens, value: 120 }],
    });

    expect(event.type).toBe('STATE_DELTA');
    if (event.type !== 'STATE_DELTA') {
      return;
    }
    expect(event.patch).toHaveLength(1);
  });
});

describe('usage lands where the service writes it', () => {
  it('folds the counts the service actually sends', () => {
    // The defect: the paths were spelled `/usage/tokens` and `/usage/cost`
    // here while the service writes `/usage/inputTokens`,
    // `/usage/outputTokens` and `/usage/contextUsed`, so every patch fell
    // through to the ignored branch.
    expect(PATHS.inputTokens).toBe('/usage/inputTokens');
    expect(PATHS.outputTokens).toBe('/usage/outputTokens');
    expect(PATHS.contextUsed).toBe('/usage/contextUsed');

    const frames = [
      { op: 'replace', path: PATHS.inputTokens, value: 1200 },
      { op: 'replace', path: PATHS.outputTokens, value: 300 },
      { op: 'replace', path: PATHS.contextUsed, value: 44 },
    ];
    let state = EMPTY;
    for (const patch of frames) {
      state = reduce(
        state,
        toEvent({
          type: 'STATE_DELTA',
          threadId: 'sesn_1',
          runId: 'run_1',
          seq: 5,
          delta: [patch],
        }),
      );
    }

    expect(state.usage?.inputTokens).toBe(1200);
    expect(state.usage?.outputTokens).toBe(300);
    expect(state.usage?.contextUsed).toBe(44);
    // §18.5 asks for the count a reader watches, not a breakdown to add up.
    expect(state.usage?.tokens).toBe(1500);
  });

  it('reports no total until the service has reported one', () => {
    // §18.5: absence is a fact, not a zero. A total shown before anything
    // arrived would be this client inventing a figure.
    expect(EMPTY.usage?.tokens).toBeUndefined();
  });
});

describe('one durable event may project to several frames', () => {
  it('keys them apart, so none is mistaken for a duplicate', () => {
    // A committed message becomes START, then CONTENT, then END, and all
    // three carry the sequence of the event they came from — that sequence
    // is the cursor a reconnect resumes on. Keyed by sequence alone, the
    // siblings were dropped and every message rendered empty.
    expect(frameKey(7, 'TEXT_MESSAGE_START')).not.toBe(frameKey(7, 'TEXT_MESSAGE_CONTENT'));
    // The overlap the dedup exists for still collapses.
    expect(frameKey(7, 'TEXT_MESSAGE_CONTENT')).toBe(frameKey(7, 'TEXT_MESSAGE_CONTENT'));
  });
});

describe('the paths match what the service emits', () => {
  /*
   * The list on the right was read out of `api/crates/layer1-public/src/agui/`
   * on 2026-08-19 by listing every `"path"` the projection writes. It is the
   * whole set, so this catches both halves of the drift that caused the
   * original defect: a path declared here that nothing writes, and a path the
   * service writes that nothing here reads.
   *
   * `/usage/cost` is the one deliberate exception and is asserted as such
   * below — declared, unwritten, and recorded as unwritten.
   */
  const EMITTED = [
    '/availableCommands',
    '/configOptions',
    '/gaps/-',
    '/mode',
    '/plan',
    '/planeChanges/-',
    '/signIn',
    '/usage/contextUsed',
    '/usage/inputTokens',
    '/usage/outputTokens',
  ];

  it('declares every path the service writes', () => {
    const declared = new Set(Object.values(PATHS));
    for (const path of EMITTED) {
      expect(declared, `the service emits ${path} and nothing here reads it`).toContain(path);
    }
  });

  it('declares nothing beyond those, but for the one recorded absence', () => {
    // A path declared here that the service never writes is a promise this
    // client cannot keep, and the reason the usage panel read zero for months.
    const extra = Object.values(PATHS).filter((path) => !EMITTED.includes(path));
    expect(extra).toEqual(['/usage/cost']);
  });
});
