import { expect, it } from 'vitest';
import { ProtocolError, ValidationError } from '../src/index.js';
import { transport } from './resource-fixtures.js';

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
