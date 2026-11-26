// @vitest-environment node
import { expect, it } from 'vitest';
import { ProtocolError } from '../index.js';
import { createRestWebClient } from '../../api/rest';
import { transport } from './resource-fixtures.js';

const ready = {
  status: 'ready',
  agent_id: 'agent-preview',
  source_through_seq: 30,
  latest_assistant_reply: {
    text: 'A retained reply from a failed or active turn.',
    truncated: false,
    message_sequence: 29,
    turn_sequence: 20,
    appended_at: '2026-11-23T10:30:00Z',
  },
};

it('reads the exact public projection without caching or any write', async () => {
  const test = transport();
  const abort = new AbortController();
  test.answer(ready);
  expect(await test.client.agents.replyPreview(ready.agent_id, { signal: abort.signal })).toEqual(
    ready,
  );
  expect(test.calls).toHaveLength(1);
  expect(test.calls[0]!.url).toBe(
    'https://cma.example.test/api/v1/agents/agent-preview/reply-preview',
  );
  expect(test.calls[0]!.init).toMatchObject({ method: 'GET', cache: 'no-store' });
  expect(test.calls[0]!.init.body).toBeUndefined();
  expect(new Headers(test.calls[0]!.init.headers).has('Idempotency-Key')).toBe(false);
});

it.each([
  { status: 'none', source_through_seq: 0 },
  { status: 'none', source_through_seq: 30 },
  { status: 'unavailable', source_through_seq: null },
  { status: 'unavailable', source_through_seq: 30 },
])('preserves absent preview evidence %j', async (fields) => {
  const test = transport();
  const response = { ...fields, agent_id: ready.agent_id, latest_assistant_reply: null };
  test.answer(response);
  expect(await test.client.agents.replyPreview(ready.agent_id)).toEqual(response);
  const result = await createRestWebClient(test.client).readAgentReplyPreview!(ready.agent_id);
  expect(result).toEqual({
    kind: fields.status,
    agentId: ready.agent_id,
    sourceThroughSeq: fields.source_through_seq,
  });
});

it('maps source identity and original reply time without inventing success or update time', async () => {
  const test = transport();
  test.answer(ready);
  expect(await createRestWebClient(test.client).readAgentReplyPreview!(ready.agent_id)).toEqual({
    kind: 'ready',
    agentId: ready.agent_id,
    sourceThroughSeq: 30,
    reply: {
      text: ready.latest_assistant_reply.text,
      truncated: false,
      messageSequence: 29,
      turnSequence: 20,
      appendedAt: '2026-11-23T10:30:00Z',
    },
  });
});

it.each([
  { agent_id: 'other-agent' },
  { status: 'unknown' },
  { status: 'none' },
  { status: 'unavailable' },
  { source_through_seq: null },
  { source_through_seq: 28 },
  { latest_assistant_reply: null },
])('rejects mismatched or contradictory evidence %j', async (change) => {
  const test = transport();
  test.answer({ ...ready, ...change });
  await expect(test.client.agents.replyPreview(ready.agent_id)).rejects.toBeInstanceOf(
    ProtocolError,
  );
});

it.each([
  { text: '' },
  { text: '   ' },
  { text: ' leading' },
  { text: 'duplicate  spaces' },
  { text: 'control\ncharacter' },
  { text: '\ud800' },
  { text: '🦊'.repeat(241) },
  { appended_at: 'not a date' },
  { turn_sequence: 29 },
  { message_sequence: 0 },
  { message_sequence: 29.1 },
  { message_sequence: Number.MAX_SAFE_INTEGER + 1 },
])('rejects malformed reply %j', async (change) => {
  const test = transport();
  test.answer({ ...ready, latest_assistant_reply: { ...ready.latest_assistant_reply, ...change } });
  await expect(test.client.agents.replyPreview(ready.agent_id)).rejects.toBeInstanceOf(
    ProtocolError,
  );
});

it('accepts 240 Unicode scalars and keeps markup as plain source text', async () => {
  const test = transport();
  for (const text of ['🦊'.repeat(240), '<img src=x> **plain source**']) {
    test.answer({ ...ready, latest_assistant_reply: { ...ready.latest_assistant_reply, text } });
    const response = await test.client.agents.replyPreview(ready.agent_id);
    expect(response.latest_assistant_reply?.text).toBe(text);
  }
});

it.each([401, 403, 404, 503])(
  'preserves HTTP %i instead of fabricating no reply',
  async (status) => {
    const test = transport();
    test.answer({ detail: 'Synthetic read refusal' }, status);
    await expect(test.client.agents.replyPreview(ready.agent_id)).rejects.toMatchObject({
      status,
    });
    expect(test.calls).toHaveLength(1);
  },
);
