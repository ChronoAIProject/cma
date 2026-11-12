import { expect, it } from 'vitest';
import { conversationFromTranscript, foldAgentEvent } from './conversation';
import { queueObservationRevision } from './queueObservationRevision';

it('hints on input and turn changes, but not on usage or assistant output', () => {
  const empty = conversationFromTranscript({ agentId: 'agt_fixture', throughSeq: 0, turns: [] });
  let state = empty;
  for (const [seq, kind] of [
    [1, { type: 'usage', input_tokens: 1, output_tokens: 1, context_used: 2 }],
    [2, { type: 'agent_message', content: 'Fixture output' }],
  ] as const) {
    state = foldAgentEvent(state, { seq, appendedAt: '2026-11-11T00:00:00Z', kind });
    expect(queueObservationRevision(state)).toBe(queueObservationRevision(empty));
  }
  state = foldAgentEvent(state, {
    seq: 3,
    appendedAt: '2026-11-11T00:00:00Z',
    kind: { type: 'owner_message', content: 'Fixture input' },
  });
  expect(queueObservationRevision(state)).not.toBe(queueObservationRevision(empty));
  const previous = queueObservationRevision(state);
  state = foldAgentEvent(state, {
    seq: 4,
    appendedAt: '2026-11-11T00:00:00Z',
    kind: { type: 'turn_started', turn_sequence: 4 },
  });
  expect(queueObservationRevision(state)).not.toBe(previous);
});
