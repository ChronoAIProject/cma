import { describe, expect, it } from 'vitest';
import { fold } from './conversation';
import type { AguiEvent } from '../agui';

/**
 * The reducer that turns an event stream into a conversation.
 *
 * Pure, and tested as such: what a client draws is a function of what the log
 * said, so the cases worth pinning are the ones where the log says something
 * happened *to* the conversation rather than in it.
 */

/**
 * A state delta as the stream carries it.
 *
 * The run and thread identifiers are on every frame and matter to nothing this
 * reducer does, so they are filled in here rather than repeated in each case.
 */
function delta(seq: number, path: string, value: Record<string, unknown>): AguiEvent {
  return {
    type: 'STATE_DELTA',
    seq,
    threadId: 'sesn_1',
    runId: 'run_1',
    patch: [{ op: 'add', path, value }],
  };
}

/** The stream as a client would have read it. */
const stream = (events: readonly AguiEvent[]) => fold(events);

describe('a plane change is a boundary', () => {
  it('draws where history stopped being continuous, and where the session went', () => {
    // §18.6: a rule across the stream at the snapshot moment, not a status
    // chip. Anything the agent did after that moment did not travel, and a
    // gap somebody can see is one they can recognise rather than read as the
    // agent having forgotten.
    const state = stream([
      delta(9, '/planeChanges/-', {
        from: 'terminal',
        to: 'conversation',
        snapshotAt: '2026-08-18T09:14:22Z',
      }),
    ]);

    const boundary = state.entries.at(-1);
    expect(boundary?.kind).toBe('gap');
    expect(boundary).toMatchObject({
      rotatedAt: '2026-08-18T09:14:22Z',
      movedTo: 'conversation',
    });
  });

  it('keeps a plane change and a lost stretch of conversation apart', () => {
    // Both are boundaries and neither is the other: one is something the owner
    // asked for and the other is something that happened to them.
    const state = stream([
      delta(4, '/gaps/-', { reason: 'behind' }),
      delta(5, '/planeChanges/-', { to: 'terminal', snapshotAt: '2026-08-18T10:00:00Z' }),
    ]);

    const gaps = state.entries.filter((entry) => entry.kind === 'gap');
    expect(gaps).toHaveLength(2);
    expect(gaps[0]?.kind === 'gap' && gaps[0].movedTo).toBeUndefined();
    expect(gaps[1]).toMatchObject({ movedTo: 'terminal' });
  });
});
