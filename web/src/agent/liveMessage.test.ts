import { expect, it } from 'vitest';
import type { AgentLiveEvent } from '../api/types';
import { conversationFromTranscript, foldAgentEvent, foldLiveEvent } from './conversation';

const empty = () => conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] });
const event = (type: string, fields: Record<string, unknown>): AgentLiveEvent => ({
  type,
  runId: '1',
  threadId: 'agt_1',
  ...fields,
});

it('renders content-first live fragments and replaces them with the complete durable projection', () => {
  let state = empty();
  state = foldLiveEvent(
    state,
    event('TEXT_MESSAGE_CONTENT', {
      'cma:live': true,
      messageId: 'opaque-message',
      delta: 'Partial',
    }),
    'agt_1',
  );
  expect(state.messages[0]).toMatchObject({
    content: 'Partial',
    liveOnly: true,
    role: 'assistant',
  });
  const order = state.messages[0]?.order;
  state = foldLiveEvent(
    state,
    event('TEXT_MESSAGE_CONTENT', {
      'cma:live': true,
      messageId: 'opaque-message',
      delta: ' text',
    }),
    'agt_1',
  );
  expect(state.messages[0]?.content).toBe('Partial text');
  for (const frame of [
    event('TEXT_MESSAGE_START', { messageId: 'msg_5', role: 'assistant' }),
    event('TEXT_MESSAGE_CONTENT', { messageId: 'msg_5', delta: 'The complete answer.' }),
    event('TEXT_MESSAGE_END', { messageId: 'msg_5' }),
  ])
    state = foldLiveEvent(state, frame, 'agt_1');
  expect(state.messages).toEqual([
    expect.objectContaining({
      id: 'msg_5',
      content: 'The complete answer.',
      status: 'complete',
      order,
    }),
  ]);
  expect(state.messages[0]?.liveOnly).toBeUndefined();
});

it('reconciles direct durable Agent messages without removing another run live block', () => {
  let state = foldAgentEvent(empty(), {
    seq: 1,
    appendedAt: '2026-09-06T00:00:00Z',
    kind: { type: 'turn_started', turn_sequence: 1 },
  });
  state = foldLiveEvent(
    state,
    event('TEXT_MESSAGE_CONTENT', { 'cma:live': true, messageId: 'opaque-one', delta: 'Partial' }),
    'agt_1',
  );
  state = foldLiveEvent(
    state,
    event('TEXT_MESSAGE_CONTENT', {
      'cma:live': true,
      messageId: 'opaque-two',
      runId: '2',
      delta: 'Another run',
    }),
    'agt_1',
  );
  state = foldAgentEvent(state, {
    seq: 2,
    appendedAt: '2026-09-06T00:00:00Z',
    kind: { type: 'agent_message', content: 'Durable answer' },
  });
  expect(state.messages.map((message) => message.content)).toEqual([
    'Durable answer',
    'Another run',
  ]);
  expect(state.messages.find((message) => message.content === 'Durable answer')?.order).toBe(1);
});

it('does not invent ordinary messages when their start frame is absent', () => {
  const state = empty();
  expect(
    foldLiveEvent(
      state,
      event('TEXT_MESSAGE_CONTENT', { messageId: 'unstarted', delta: 'Missing start' }),
      'agt_1',
    ),
  ).toBe(state);
});
