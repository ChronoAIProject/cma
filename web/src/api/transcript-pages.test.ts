/** @vitest-environment node */
import { expect, it } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { NOW, restTransport } from '../test/rest';
import { createRestWebClient } from './rest';

const turn = {
  turn_sequence: 3,
  started_at: NOW,
  owner_message: 'owner',
  outcome: { type: 'completed', at: NOW },
};
const owner = { sequence: 4, role: 'user', content: 'owner', appended_at: NOW };
const answer = { sequence: 35, role: 'assistant', content: 'answer', appended_at: NOW };

it('hydrates the browser transcript through bounded pages inside a long turn', async () => {
  const test = restTransport();
  test.reply({ code: 'transcript_pagination_required', detail: 'Use pages' }, 400);
  test.reply({
    agent_id: 'agent',
    through_seq: 40,
    turns: [{ ...turn, agent_messages: [], messages: [owner] }],
    next: { turn: 3, after: 34, through: 40, limit: 32 },
  });
  test.reply({
    agent_id: 'agent',
    through_seq: 40,
    turns: [{ ...turn, agent_messages: ['answer'], messages: [answer] }],
  });
  const result = await createRestWebClient(test.sdk).readAgentTranscript('agent');
  expect(result.throughSeq).toBe(40);
  expect(result.turns).toHaveLength(1);
  expect(result.turns[0]?.messages.map((m) => m.sequence)).toEqual([4, 35]);
  expect(result.turns[0]?.agentMessages).toEqual(['answer']);
  expect(test.fetch).toHaveBeenCalledTimes(3);
});

it('rejects partial browser hydration when continuation authority is revoked', async () => {
  const test = restTransport();
  test.reply({ code: 'transcript_pagination_required', detail: 'Use pages' }, 400);
  test.reply({
    agent_id: 'agent',
    through_seq: 40,
    turns: [{ ...turn, agent_messages: [], messages: [owner] }],
    next: { turn: 3, after: 34, through: 40, limit: 32 },
  });
  test.reply({ code: 'forbidden', detail: 'Revoked' }, 403);
  await expect(createRestWebClient(test.sdk).readAgentTranscript('agent')).rejects.toBeInstanceOf(
    ApiError,
  );
  expect(test.fetch).toHaveBeenCalledTimes(3);
});
