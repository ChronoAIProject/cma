import { expect, it } from 'vitest';
import { ProtocolError, ValidationError } from '../src/index.js';
import { transport } from './resource-fixtures.js';

it('reads reason versions and saves six reasons with an exact conditional request', async () => {
  const test = transport();
  const current = {
    viewer_id: 'owner',
    agent_id: 'agent-one',
    message_id: 'event:4',
    vote: 'down',
    version: 1,
    reasons: [],
    comment: '',
    updated_at: null,
    request_id: null,
  };
  test.answer(current);
  expect(await test.client.agents.feedbackDetails('agent-one', 'event:4')).toEqual(current);
  const body = {
    expected_version: 1,
    request_id: 'reason-request',
    reasons: [
      'inaccurate',
      'unanswered',
      'unusable_steps',
      'incomplete',
      'unclear',
      'other',
    ] as const,
    comment: '🦀'.repeat(500),
  };
  const saved = { ...current, ...body, version: 2, updated_at: '2026-11-03T01:00:00Z' };
  test.answer(saved);
  expect(await test.client.agents.setFeedbackReasons('agent-one', 'event:4', body)).toMatchObject({
    version: 2,
    reasons: body.reasons,
    comment: body.comment,
  });
  expect(JSON.parse(String(test.calls.at(-1)?.init.body))).toEqual(body);
  expect(test.calls.at(-1)?.url).toContain('/messages/event:4/feedback/reasons');
});

it('reads paginated votes and sets, switches and clears exact message metadata without a command key', async () => {
  const test = transport();
  const page = { items: [{ message_id: 'event:4', vote: 'up' }], next: 4 };
  test.answer(page);
  expect(await test.client.agents.messageFeedback('agent-one', { after: 2, limit: 1 })).toEqual(
    page,
  );
  expect(test.calls[0]?.url).toContain('/api/v1/agents/agent-one/message-feedback?after=2&limit=1');
  for (const vote of ['up', 'down', null] as const) {
    test.answer({ message_id: 'event:4', vote });
    expect(await test.client.agents.setMessageFeedback('agent-one', 'event:4', vote)).toEqual({
      message_id: 'event:4',
      vote,
    });
    const call = test.calls.at(-1)!;
    expect(call.url).toContain('/messages/event:4/feedback');
    expect(call.init.method).toBe('PUT');
    expect(JSON.parse(String(call.init.body))).toEqual({ vote });
    expect(new Headers(call.init.headers).has('Idempotency-Key')).toBe(false);
  }
});
it('rejects non-durable identities and mismatched receipts', async () => {
  const test = transport();
  for (const id of ['event:0', 'event:04', 'optimistic:4'])
    expect(() => test.client.agents.setMessageFeedback('agent', id, 'up')).toThrow(ValidationError);
  expect(test.calls).toHaveLength(0);
  test.answer({ message_id: 'event:5', vote: 'up' });
  await expect(
    test.client.agents.setMessageFeedback('agent', 'event:4', 'up'),
  ).rejects.toBeInstanceOf(ProtocolError);
});
