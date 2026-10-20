import { expect, it } from 'vitest';
import type { V2AgentCursor, V2AgentStreamFrame } from '@chronoai/cma-sdk';
import { conversationFromTranscript } from '../conversation';
import { foldV2Frame, foldV2Item } from './projection';

it('keeps the first real server live observation and replaces it with recorded message time', () => {
  const incarnation = 'a'.repeat(64);
  let state = {
    ...conversationFromTranscript({ agentId: 'agent', throughSeq: 2, turns: [] }),
    agentIncarnation: incarnation,
    activeRunId: '1',
  };
  const live = (at?: string): V2AgentStreamFrame => ({
    type: 'cma.agent.live',
    data: {
      agent_id: 'agent',
      incarnation,
      turn_sequence: 1,
      after_sequence: 2,
      message_id: 'live-one',
      kind: 'text_delta',
      delta: 'part',
      observed_at: at,
    },
  });
  state = { ...state, ...foldV2Frame(state, live(), 'agent') };
  expect(state.messages[0]?.at).toBeUndefined();
  state = { ...state, ...foldV2Frame(state, live('2026-10-20T16:31:00Z'), 'agent') };
  expect(state.messages[0]?.at).toBe('2026-10-20T16:31:00Z');
  state = { ...state, ...foldV2Frame(state, live('2026-10-20T16:32:00Z'), 'agent') };
  expect(state.messages[0]?.at).toBe('2026-10-20T16:31:00Z');
  state = {
    ...state,
    ...foldV2Item(state, {
      sequence: 3,
      cursor: `ag1.${incarnation}.3` as V2AgentCursor,
      appended_at: '2026-10-20T16:33:00Z',
      response_binding: null,
      kind: { type: 'message', role: 'assistant', content: 'partpartpart' },
    }),
  };
  expect(state.messages).toHaveLength(1);
  expect(state.messages[0]).toMatchObject({ id: 'event:3', at: '2026-10-20T16:33:00Z' });
});
