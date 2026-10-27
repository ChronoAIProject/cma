import { expect, it } from 'vitest';
import type { V2AgentCursor, V2AgentEventItem } from '@chronoai/cma-sdk';
import { conversationFromTranscript } from '../conversation';
import { foldV2Item } from './projection';
const row = (
  sequence: number,
  kind: V2AgentEventItem['kind'],
  extra: Partial<V2AgentEventItem> = {},
): V2AgentEventItem => ({
  sequence,
  cursor: `ag1.${'a'.repeat(64)}.${sequence}` as V2AgentCursor,
  appended_at: '2026-10-16T00:00:00Z',
  response_binding: null,
  kind,
  ...extra,
});
const start = () =>
  foldV2Item(
    conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] }),
    row(
      1,
      { type: 'turn_started', turn_sequence: 1 },
      {
        response_binding: {
          command_id: 'cmd_original',
          response_id: 'resp_original',
          turn_sequence: 1,
        },
      },
    ),
  );
it('binds each same-turn message to its own exact command even when text is identical', () => {
  let state = foldV2Item(start(), row(2, { type: 'message', role: 'user', content: 'same text' }));
  state = foldV2Item(
    state,
    row(3, {
      type: 'delivery_changed',
      operation: 'steer',
      operation_sequence: 3,
      state: 'dispatch_fenced',
    }),
  );
  state = foldV2Item(
    state,
    row(
      4,
      { type: 'message', role: 'user', content: 'same text' },
      { message_command_id: 'cmd_steer_one' },
    ),
  );
  const last = row(
    6,
    { type: 'message', role: 'user', content: 'same text' },
    { message_command_id: 'cmd_steer_two' },
  );
  state = foldV2Item(state, last);
  expect(state.messages.map((message) => message.commandId)).toEqual([
    'cmd_original',
    'cmd_steer_one',
    'cmd_steer_two',
  ]);
  expect(foldV2Item(state, last)).toBe(state);
});
it('keeps legacy steering unbound instead of assigning the original response command', () => {
  let state = foldV2Item(start(), row(2, { type: 'message', role: 'user', content: 'original' }));
  state = foldV2Item(state, row(4, { type: 'message', role: 'user', content: 'legacy steering' }));
  expect(state.messages.map((message) => message.commandId)).toEqual(['cmd_original', undefined]);
});

it('reconciles only the optimistic steering row with the exact recorded command identity', async () => {
  const { act, renderHook } = await import('@testing-library/react');
  const { useOptimisticMessages } = await import('../optimisticMessages');
  const identity = {};
  const original = foldV2Item(
    start(),
    row(2, { type: 'message', role: 'user', content: 'same text' }),
  );
  const hook = renderHook(
    ({ conversation }) => useOptimisticMessages(identity, true, conversation, undefined),
    { initialProps: { conversation: original } },
  );
  act(() => {
    hook.result.current
      .begin('same text')
      .accepted({ commandId: 'cmd_steer_one', responseId: 'resp_original' });
    hook.result.current
      .begin('same text')
      .accepted({ commandId: 'cmd_steer_two', responseId: 'resp_original' });
  });
  expect(hook.result.current.messages).toHaveLength(2);
  hook.rerender({
    conversation: foldV2Item(
      original,
      row(
        4,
        { type: 'message', role: 'user', content: 'same text' },
        { message_command_id: 'cmd_steer_one' },
      ),
    ),
  });
  expect(hook.result.current.messages).toHaveLength(1);
  expect(hook.result.current.messages[0]?.receipt).toEqual({
    commandId: 'cmd_steer_two',
    responseId: 'resp_original',
  });
});
