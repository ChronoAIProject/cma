import { expect, it } from 'vitest';
import { conversationFromTranscript, foldLiveEvent } from './conversation';
import type { ConversationState } from './model';
import { orderEntries } from './chronology';

function history(count: number): ConversationState {
  return {
    ...conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] }),
    messages: Array.from({ length: count }, (_, index) => ({
      id: `event:${index + 1}`,
      runId: String(index),
      role: 'assistant',
      content: `Recorded reply ${index}`,
      status: 'complete',
      order: index + 1,
    })),
    toolCalls: [{ id: 'tool', title: 'Read file', status: 'completed', order: count + 1 }],
    approvals: [{ id: 'approval', reason: 'Write file', decision: 'approved', order: count + 2 }],
  };
}

it('bounds ten thousand historical rows while preserving recent rows and unrelated groups', () => {
  const recorded = history(10_000);
  let state = foldLiveEvent(
    recorded,
    {
      type: 'TEXT_MESSAGE_START',
      threadId: 'agent',
      runId: 'live',
      messageId: 'reply',
      role: 'assistant',
    },
    'agent',
  );
  const baseline = state;
  for (const type of ['TEXT_MESSAGE_CONTENT', 'REASONING_MESSAGE_CONTENT'])
    for (let index = 0; index < 20; index++)
      state = foldLiveEvent(
        state,
        {
          type,
          threadId: 'agent',
          runId: 'live',
          messageId: type.startsWith('TEXT') ? 'reply' : 'thinking',
          delta: 'More text. ',
        },
        'agent',
      );
  expect(state.messages.length + state.toolCalls.length + state.reasoning.length).toBe(200);
  const retained = state.messages.slice(0, -1);
  expect(retained).toEqual(recorded.messages.slice(-retained.length));
  expect(retained.every((item) => recorded.messages.includes(item))).toBe(true);
  expect(state.displayLimited).toBe(true);
  expect(state.toolCalls).toBe(baseline.toolCalls);
  expect(state.approvals).toBe(baseline.approvals);
  expect(state.plans).toBe(baseline.plans);
  expect(state.recovery).toBe(baseline.recovery);
  expect(state.messages.at(-1)?.content).toBe('More text. '.repeat(20));
  expect(state.reasoning[0]?.content).toBe('More text. '.repeat(20));
  expect(baseline.messages.at(-1)?.content).toBe('');
});

it('keeps chronological anchors when a tool settles and a live row becomes durable', () => {
  const recorded = history(2);
  let state = foldLiveEvent(
    recorded,
    {
      type: 'TOOL_CALL_START',
      threadId: 'agent',
      runId: 'live',
      toolCallId: 'pending',
    },
    'agent',
  );
  state = foldLiveEvent(
    state,
    {
      type: 'TEXT_MESSAGE_CONTENT',
      threadId: 'agent',
      runId: 'live',
      messageId: 'reply',
      delta: 'Text',
      'cma:live': true,
    },
    'agent',
  );
  const reply = state.messages.at(-1)!;
  state = foldLiveEvent(
    state,
    {
      type: 'TOOL_CALL_END',
      threadId: 'agent',
      runId: 'live',
      toolCallId: 'pending',
      cursor: 10,
    },
    'agent',
  );
  expect(state.toolCalls.at(-1)?.order).toBeGreaterThan(reply.order!);
  expect(state.toolCalls.at(-1)?.sequence).toBe(10);
  expect(state.messages.at(-1)).toBe(reply);
  state = foldLiveEvent(
    state,
    {
      type: 'TEXT_MESSAGE_START',
      threadId: 'agent',
      runId: 'live',
      messageId: 'durable',
      role: 'assistant',
      cursor: 11,
    },
    'agent',
  );
  expect(state.messages.at(-1)).toMatchObject({ id: 'durable', order: reply.order, sequence: 11 });
  expect(state.messages[0]).toBe(recorded.messages[0]);
});

it('recomputes order bounds after explicit history insertion and removal', () => {
  const baseline = history(2);
  const inserted = orderEntries(baseline, {
    ...baseline,
    messages: [
      ...baseline.messages,
      { id: 'imported', role: 'user', content: 'Recovered', status: 'complete', order: 100 },
    ],
  });
  const removed = orderEntries(inserted, { ...inserted, messages: inserted.messages.slice(0, 2) });
  for (const [before, expected] of [
    [inserted, 101],
    [removed, 5],
  ] as const) {
    const next = foldLiveEvent(
      before,
      {
        type: 'TEXT_MESSAGE_START',
        threadId: 'agent',
        runId: 'live',
        messageId: 'new',
      },
      'agent',
    );
    expect(next.messages.at(-1)?.order).toBe(expected);
  }
});
