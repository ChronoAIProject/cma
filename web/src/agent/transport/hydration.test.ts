import { expect, it } from 'vitest';
import type { V2AgentStreamFrame } from '@chronoai/cma-sdk';
import { conversationFromTranscript } from '../conversation';
import type { ConversationState } from '../model';
import { ConversationHydration } from './hydration';
import { foldV2Item } from './projection';
import { at, binding, cursor, incarnation } from './controller.fixture';

const empty = () => ({
  ...conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] }),
  agentCursor: cursor(0),
  agentIncarnation: incarnation,
});
function durable(sequence: number, kind: unknown, response = null as unknown): V2AgentStreamFrame {
  return {
    type: 'cma.agent.event',
    data: {
      agent_id: 'agt_1',
      incarnation,
      cursor: cursor(sequence),
      sequence,
      appended_at: at,
      kind,
      response_binding: response,
    },
  } as V2AgentStreamFrame;
}
function live(delta: string, kind = 'text_delta'): V2AgentStreamFrame {
  return {
    type: 'cma.agent.live',
    data: {
      agent_id: 'agt_1',
      incarnation,
      turn_sequence: 1,
      after_sequence: 2,
      message_id: kind,
      kind,
      delta,
    },
  } as V2AgentStreamFrame;
}

it('coalesces fragments and never resurrects a reply already represented by a later snapshot', () => {
  const hydration = new ConversationHydration();
  let state = hydration.merge(empty(), 'agt_1', false);
  const frames = [
    durable(1, { type: 'turn_started', turn_sequence: 1 }, binding),
    durable(2, { type: 'message', role: 'user', content: 'Question' }),
    live('First '),
    live('answer'),
    live('Thought', 'reasoning_summary_delta'),
  ];
  for (const frame of frames) state = hydration.applyV2(state, frame, 'agt_1', true);
  expect(state.messages.at(-1)?.content).toBe('First answer');
  let snapshot = empty();
  for (const frame of [
    ...frames.slice(0, 2),
    durable(3, { type: 'message', role: 'assistant', content: 'Final answer' }),
    durable(4, { type: 'turn_completed', turn_sequence: 1 }),
  ])
    if (frame.type === 'cma.agent.event')
      snapshot = {
        ...foldV2Item(snapshot, frame.data),
        agentCursor: frame.data.cursor,
        agentIncarnation: incarnation,
      };
  const merged = hydration.merge(snapshot, 'agt_1', true);
  expect(merged.messages.map((message) => message.content)).toEqual(['Question', 'Final answer']);
  expect(merged.reasoning).toEqual([]);
  expect(merged.activeRunId).toBeUndefined();
});

it('keeps live state when an older transcript arrives and replaces it only with complete rich history', () => {
  const hydration = new ConversationHydration();
  let state = hydration.merge(empty(), 'agt_1', false);
  state = hydration.applyV2(
    state,
    durable(1, { type: 'turn_started', turn_sequence: 1 }, binding),
    'agt_1',
    true,
  );
  state = hydration.applyV2(
    state,
    durable(2, { type: 'message', role: 'user', content: 'New question' }),
    'agt_1',
    true,
  );
  hydration.applyV2(state, live('New answer'), 'agt_1', true);
  const merged = hydration.merge(
    conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] }),
    'agt_1',
    false,
  );
  expect(merged.messages.map((message) => message.content)).toEqual(['New question', 'New answer']);
  expect(merged.activeRunId).toBe('1');
  expect(merged.agentCursor).toBe(cursor(2));
  expect(merged.responseBinding?.commandId).toBe('cmd_1');
});

it('refuses a changed conversation incarnation and clears buffered output on reset', () => {
  const hydration = new ConversationHydration();
  hydration.merge(empty(), 'agt_1', false);
  expect(() =>
    hydration.merge({ ...empty(), agentIncarnation: 'b'.repeat(64) }, 'agt_1', true),
  ).toThrow('identity');
  hydration.reset();
  expect(hydration.merge(empty(), 'agt_1', true).messages).toEqual([]);
});

function completedSnapshot(): ConversationState {
  let snapshot: ConversationState = empty();
  for (const frame of [
    durable(1, { type: 'turn_started', turn_sequence: 1 }, binding),
    durable(2, { type: 'message', role: 'user', content: 'Question' }),
    durable(3, { type: 'message', role: 'assistant', content: 'Final answer' }),
    durable(4, { type: 'turn_completed', turn_sequence: 1 }),
  ])
    if (frame.type === 'cma.agent.event') snapshot = foldV2Item(snapshot, frame.data);
  return snapshot;
}

it.each([false, true])(
  'ignores late old-turn fragments after newer history (next turn in snapshot: %s)',
  (nextTurnInSnapshot) => {
    const hydration = new ConversationHydration();
    let state = hydration.merge(empty(), 'agt_1', false);
    for (const frame of [
      durable(1, { type: 'turn_started', turn_sequence: 1 }, binding),
      durable(2, { type: 'message', role: 'user', content: 'Question' }),
    ])
      state = hydration.applyV2(state, frame, 'agt_1', true);
    const nextTurn = [
      durable(5, { type: 'turn_started', turn_sequence: 5 }),
      durable(6, { type: 'message', role: 'user', content: 'Next question' }),
    ];
    let snapshot = completedSnapshot();
    if (nextTurnInSnapshot)
      for (const frame of nextTurn)
        if (frame.type === 'cma.agent.event') snapshot = foldV2Item(snapshot, frame.data);
    state = hydration.merge(snapshot, 'agt_1', true);
    expect(hydration.applyV2(state, live('Late answer'), 'agt_1', false)).toBe(state);
    expect(
      hydration.applyV2(state, live('Late thought', 'reasoning_summary_delta'), 'agt_1', false),
    ).toBe(state);
    for (const frame of nextTurn) state = hydration.applyV2(state, frame, 'agt_1', false);
    const nextLive = live('Next answer');
    if (nextLive.type !== 'cma.agent.live') throw new Error('Expected a live fixture');
    state = hydration.applyV2(
      state,
      { ...nextLive, data: { ...nextLive.data, turn_sequence: 5, after_sequence: 6 } },
      'agt_1',
      false,
    );
    const beforeGap = state;
    state = hydration.applyV2(
      state,
      {
        type: 'cma.agent.live_gap',
        data: {
          agent_id: 'agt_1',
          incarnation,
          turn_sequence: 1,
          frames: 1,
          reason: 'lagged',
        },
      },
      'agt_1',
      false,
    );
    expect(state).toBe(beforeGap);
    expect(state.activeRunId).toBe('5');
    expect(state.messages.map((message) => message.content)).toEqual([
      'Question',
      'Final answer',
      'Next question',
      'Next answer',
    ]);
    state = hydration.applyV2(
      state,
      durable(7, { type: 'message', role: 'assistant', content: 'Next answer' }),
      'agt_1',
      false,
    );
    state = hydration.applyV2(
      state,
      durable(8, { type: 'turn_completed', turn_sequence: 5 }),
      'agt_1',
      false,
    );
    expect(state.activeRunId).toBeUndefined();
    expect(state.messages.filter((message) => message.content === 'Next answer')).toHaveLength(1);
  },
);

it('still rejects identity changes and invalid live frames at the authoritative boundary', () => {
  const hydration = new ConversationHydration();
  const state = hydration.merge(completedSnapshot(), 'agt_1', true);
  const frame = live('Invalid fragment');
  if (frame.type !== 'cma.agent.live') throw new Error('Expected a live fixture');
  for (const data of [
    { ...frame.data, agent_id: 'agt_other' },
    { ...frame.data, incarnation: 'b'.repeat(64) },
  ])
    expect(() => hydration.applyV2(state, { ...frame, data }, 'agt_1', false)).toThrow('identity');
  for (const after_sequence of [4, 5])
    expect(() =>
      hydration.applyV2(
        state,
        { ...frame, data: { ...frame.data, after_sequence } },
        'agt_1',
        false,
      ),
    ).toThrow('observed Agent turn');
  hydration.reset();
  expect(() => hydration.applyV2(state, frame, 'agt_1', false)).toThrow('observed Agent turn');
});
