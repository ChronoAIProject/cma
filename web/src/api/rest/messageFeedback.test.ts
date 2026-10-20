import { afterEach, expect, it, vi } from 'vitest';
import { messageFeedbackClient } from './messageFeedback';

afterEach(() => vi.restoreAllMocks());

it('reads bounded public feedback pages and sends exact set/clear votes with same-origin authentication', async () => {
  const fetch = vi
    .spyOn(globalThis, 'fetch')
    .mockResolvedValueOnce(
      Response.json({ items: [{ message_id: 'event:12', vote: 'up' }], next: 12 }),
    )
    .mockResolvedValueOnce(
      Response.json({ items: [{ message_id: 'event:16', vote: 'down' }], next: null }),
    )
    .mockResolvedValueOnce(Response.json({ message_id: 'event:16', vote: null }));
  const client = messageFeedbackClient();
  await expect(client.readMessageFeedback!('agent')).resolves.toEqual([
    { message_id: 'event:12', vote: 'up' },
    { message_id: 'event:16', vote: 'down' },
  ]);
  expect(fetch.mock.calls[0]?.[0]).toBe('/api/v1/agents/agent/message-feedback?limit=200');
  expect(fetch.mock.calls[1]?.[0]).toBe('/api/v1/agents/agent/message-feedback?limit=200&after=12');
  await expect(client.setMessageFeedback!('agent', 'event:16', null)).resolves.toEqual({
    message_id: 'event:16',
    vote: null,
  });
  expect(fetch).toHaveBeenLastCalledWith(
    '/api/v1/agents/agent/messages/event%3A16/feedback',
    expect.objectContaining({
      method: 'PUT',
      credentials: 'same-origin',
      cache: 'no-store',
      body: '{"vote":null}',
    }),
  );
});

it.each([
  { items: [{ message_id: 'event:12', vote: 'bad' }], next: null },
  { items: [{ message_id: 'live:12', vote: 'up' }], next: null },
  { items: [{ message_id: 'event:12', vote: null }], next: null },
  { items: [], next: 0 },
  { items: [], next: -2 },
])('rejects invalid feedback and cursors', async (body) => {
  vi.spyOn(globalThis, 'fetch').mockResolvedValue(Response.json(body));
  await expect(messageFeedbackClient().readMessageFeedback!('agent')).rejects.toThrow(
    'invalid message feedback',
  );
});

it('rejects a successful-looking write to a different message or with a different vote', async () => {
  const fetch = vi
    .spyOn(globalThis, 'fetch')
    .mockResolvedValue(Response.json({ message_id: 'event:17', vote: 'up' }));
  await expect(
    messageFeedbackClient().setMessageFeedback!('agent', 'event:16', 'up'),
  ).rejects.toThrow('different message feedback');
  fetch.mockResolvedValue(Response.json({ message_id: 'event:16', vote: 'down' }));
  await expect(
    messageFeedbackClient().setMessageFeedback!('agent', 'event:16', 'up'),
  ).rejects.toThrow('different message feedback');
});
