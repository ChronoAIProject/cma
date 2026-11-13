import { describe, expect, it } from 'vitest';
import { ApiError, ProtocolError, RequestAborted, ValidationError } from '../src/index.js';
import { now, transport } from './resource-fixtures.js';

const turn = {
  turn_sequence: 3,
  started_at: now,
  owner_message: 'owner',
  outcome: { type: 'completed', at: now },
};
const owner = { sequence: 4, role: 'user', content: 'owner', appended_at: now };
const answer = { sequence: 35, role: 'assistant', content: 'answer', appended_at: now };
const pages = [
  {
    agent_id: 'agent',
    through_seq: 80,
    turns: [{ ...turn, agent_messages: [], messages: [owner] }],
    next: { turn: 3, after: 34, through: 80, limit: 32 },
  },
  {
    agent_id: 'agent',
    through_seq: 80,
    turns: [{ ...turn, agent_messages: ['answer'], messages: [answer] }],
    next: { turn: 3, after: 66, through: 80, limit: 32 },
  },
  { agent_id: 'agent', through_seq: 80, turns: [{ ...turn, agent_messages: [], messages: [] }] },
];
function scripted(values: readonly unknown[], failure = -1) {
  const test = transport();
  let n = -1;
  test.fetch.mockImplementation((url, init) => {
    test.calls.push({ url: String(url), init: init ?? {} });
    const i = n++;
    return Promise.resolve(
      new Response(
        JSON.stringify(
          i < 0 ? { code: 'transcript_pagination_required', detail: 'Use pages' } : values[i],
        ),
        {
          status: i < 0 ? 400 : i === failure ? 403 : 200,
          headers: { 'content-type': 'application/problem+json' },
        },
      ),
    );
  });
  return test;
}

describe('transcript continuation', () => {
  it('hydrates long turns and empty source pages without duplicate messages', async () => {
    const test = scripted(pages);
    expect(await test.client.agents.transcript('agent')).toEqual({
      agent_id: 'agent',
      through_seq: 80,
      turns: [{ ...turn, messages: [owner, answer], agent_messages: ['answer'] }],
    });
    expect(test.calls.map((c) => new URL(c.url).search)).toEqual([
      '',
      '?limit=32',
      '?turn=3&after=34&through=80&limit=32',
      '?turn=3&after=66&through=80&limit=32',
    ]);
  });
  it('refuses snapshot drift, nonadvancing cursors, changed metadata and repeated messages', async () => {
    for (const bad of [
      { ...pages[1], through_seq: 81 },
      { ...pages[1], next: { turn: 3, after: 34, through: 80, limit: 32 } },
      {
        ...pages[1],
        turns: [
          { ...turn, owner_message: 'changed', messages: [answer], agent_messages: ['answer'] },
        ],
      },
      { ...pages[1], turns: [{ ...turn, messages: [owner], agent_messages: [] }] },
    ]) {
      const test = scripted([pages[0], bad]);
      await expect(test.client.agents.transcript('agent')).rejects.toBeInstanceOf(ProtocolError);
      expect(test.calls).toHaveLength(3);
    }
  });
  it('propagates revocation and cancellation instead of returning partial history', async () => {
    const test = scripted([pages[0], { code: 'forbidden', detail: 'Revoked' }], 1);
    await expect(test.client.agents.transcript('agent')).rejects.toBeInstanceOf(ApiError);
    expect(test.calls).toHaveLength(3);
    const aborted = new AbortController();
    aborted.abort();
    await expect(
      test.client.agents.transcriptPage('agent', { signal: aborted.signal }),
    ).rejects.toBeInstanceOf(RequestAborted);
    expect(test.calls).toHaveLength(3);
  });
  it('validates bounds before sending an explicit page and accepts a legacy complete answer', async () => {
    const test = transport();
    for (const limit of [0, 33, 0.5])
      await expect(test.client.agents.transcriptPage('agent', { limit })).rejects.toBeInstanceOf(
        ValidationError,
      );
    expect(test.calls).toHaveLength(0);
    const complete = { agent_id: 'agent', through_seq: 80, turns: [] };
    test.answer(complete);
    expect(await test.client.agents.transcript('agent')).toEqual(complete);
    expect(test.calls).toHaveLength(1);
  });
});
