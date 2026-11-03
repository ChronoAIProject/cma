import { describe, expect, it } from 'vitest';
import type { V2AgentEventItem } from '@chronoai/cma-sdk';
import { conversationFromTranscript, foldAgentEvent, foldLiveEvent } from './conversation';
import { mergeTranscriptSnapshot } from './chronology';
import { setPlan } from './plans';
import { foldV2Item } from './transport/projection';

const empty = () => conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] });
const started = () =>
  foldAgentEvent(empty(), {
    seq: 1,
    appendedAt: '2026-10-21T00:00:00Z',
    kind: { type: 'turn_started', turn_sequence: 1 },
  });
const steps = [
  { title: 'Read lemma 4.1 proof', status: 'completed' },
  { title: 'Draft bound for 4.2', status: 'in_progress' },
];

describe('setPlan', () => {
  it('keeps titled steps and drops malformed ones', () => {
    const state = setPlan(empty(), '1', [
      steps[0],
      { title: 'No status' },
      'not an entry',
      { title: 7, status: 'pending' },
    ]);
    expect(state.plans).toEqual([
      {
        id: 'plan:1',
        runId: '1',
        incomplete: true,
        entries: [{ title: 'Read lemma 4.1 proof', status: 'completed' }],
      },
    ]);
  });

  it('ignores a report that is not a list', () => {
    const state = empty();
    expect(setPlan(state, '1', { entries: steps })).toBe(state);
    expect(setPlan(state, '1', undefined)).toBe(state);
  });

  it('replaces a turn plan in place and keeps other turns apart', () => {
    let state = setPlan(empty(), '1', steps);
    state = setPlan(state, '2', [steps[0]]);
    state = setPlan(state, '1', [{ title: 'Run lean check', status: 'pending' }]);
    expect(state.plans.map(({ id, entries }) => [id, entries.length])).toEqual([
      ['plan:1', 1],
      ['plan:2', 1],
    ]);
    expect(setPlan(state, undefined, steps).plans.at(-1)?.id).toBe('plan:unattributed');
  });
});

describe('plan folding', () => {
  it('reads a durable plan_set into the active turn at its stream position', () => {
    const state = foldAgentEvent(started(), {
      seq: 2,
      appendedAt: '2026-10-21T00:00:01Z',
      kind: { type: 'plan_set', entries: steps },
    });
    expect(state.plans).toEqual([
      expect.objectContaining({ id: 'plan:1', runId: '1', entries: steps, sequence: 2 }),
    ]);
  });

  it('reconciles the live /plan state with the durable report for the same turn', () => {
    let state = foldLiveEvent(
      started(),
      {
        type: 'STATE_DELTA',
        threadId: 'agt_1',
        runId: '1',
        delta: [{ op: 'replace', path: '/plan', value: steps }],
      },
      'agt_1',
    );
    const order = state.plans[0]?.order;
    state = foldAgentEvent(state, {
      seq: 3,
      appendedAt: '2026-10-21T00:00:02Z',
      kind: { type: 'plan_set', entries: [{ ...steps[1]!, status: 'completed' }] },
    });
    expect(state.plans).toHaveLength(1);
    expect(state.plans[0]).toMatchObject({ order, entries: [{ status: 'completed' }] });
    const cleared = foldLiveEvent(
      state,
      {
        type: 'STATE_DELTA',
        threadId: 'agt_1',
        runId: '1',
        delta: [{ op: 'remove', path: '/plan' }],
      },
      'agt_1',
    );
    expect(cleared.plans[0]?.entries).toEqual([]);
  });

  it('reads the Responses v2 plan_set kind', () => {
    const state = foldV2Item(started(), {
      sequence: 2,
      appended_at: '2026-10-21T00:00:01Z',
      kind: { type: 'plan_set', entries: steps },
    } as unknown as V2AgentEventItem);
    expect(state.plans[0]?.entries).toEqual(steps);
  });

  it('keeps a plan in place when a transcript snapshot is merged', () => {
    const state = foldAgentEvent(started(), {
      seq: 2,
      appendedAt: '2026-10-21T00:00:01Z',
      kind: { type: 'plan_set', entries: steps },
    });
    const merged = mergeTranscriptSnapshot(
      state,
      conversationFromTranscript({ agentId: 'agt_1', throughSeq: 2, turns: [] }),
    );
    expect(merged.plans).toEqual([expect.objectContaining({ id: 'plan:1', entries: steps })]);
    expect(merged.plans[0]?.order).toBeGreaterThan(0);
  });
});
