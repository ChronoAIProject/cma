import { describe, expect, it, vi } from 'vitest';
import { Cmaeg, ProtocolError, RequestAborted, ValidationError, type cmaeg } from '../src/index.js';

const page: cmaeg.ActivityPage = {
  cursor: 'opaque/+?snapshot',
  retention_days: 7,
  limit: 50,
  has_more: false,
  items: [
    {
      event_id: 'event-1',
      source: { type: 'custom' },
      actor: { id: 'person', kind: 'human' },
      conversation: { id: 'conversation', kind: 'private' },
      occurred_at: '2026-10-27T01:00:00Z',
      received_at: '2026-10-27T01:00:01Z',
      consumers: [
        {
          consumer_id: 'consumer-1',
          state: 'completed',
          display_name: '助手',
          agent_id: null,
          deliveries: [
            {
              step: 'reply',
              destination: 'telegram',
              outcome: 'delivered',
              observed_at: '2026-10-27T01:00:02Z',
              confirmed_at: '2026-10-27T01:00:02Z',
              platform_message_id: 'message-1',
              recipients: [{ kind: 'chat', id: 'chat-1', thread_id: null }],
            },
          ],
        },
      ],
    },
  ],
};

function stream(body: string) {
  const bytes = new TextEncoder().encode(body);
  return new Response(
    new ReadableStream({
      start(controller) {
        for (let offset = 0; offset < bytes.length; offset += 3)
          controller.enqueue(bytes.slice(offset, offset + 3));
        controller.close();
      },
    }),
    { headers: { 'content-type': 'text/event-stream' } },
  );
}
const frame = (event: string, value = page, id: string | undefined = value.cursor) =>
  `event: ${event}\r\n${id === undefined ? '' : `id: ${id}\r\n`}data: ${JSON.stringify(value)}\r\n\r\n`;

describe('gateway recent Activity', () => {
  it('reads snapshots and observes full replacements with opaque resume and owner authority', async () => {
    const replacement = { ...page, cursor: 'new-snapshot', items: [] };
    const fetch = vi
      .fn<typeof globalThis.fetch>()
      .mockResolvedValueOnce(
        new Response(JSON.stringify(page), { headers: { 'content-type': 'application/json' } }),
      )
      .mockResolvedValueOnce(
        stream(`: keepalive\r\n\r\n${frame('activity')}${frame('reset', replacement)}`),
      );
    const client = new Cmaeg({
      baseUrl: 'https://gateway.example/prefix',
      credential: () => 'creator',
      fetch,
    });
    expect(await client.getChannelActivity('opaque/+?', { owner: 'org-id' })).toEqual(page);
    expect(fetch.mock.calls[0]?.[0]).toBe(
      'https://gateway.example/prefix/v1/channels/opaque%2F%2B%3F/activity',
    );
    expect(new Headers(fetch.mock.calls[0]?.[1]?.headers).get('x-cmaeg-owner')).toBe('org-id');
    const events = [];
    for await (const event of client.streamChannelActivity('opaque/+?', {
      cursor: page.cursor,
      lastEventId: 'header-cursor',
    }))
      events.push(event);
    expect(events).toEqual([
      { event: 'activity', id: page.cursor, data: page },
      { event: 'reset', id: replacement.cursor, data: replacement },
    ]);
    const request = fetch.mock.calls[1];
    expect(request?.[0]).toBe(
      'https://gateway.example/prefix/v1/channels/opaque%2F%2B%3F/activity/stream?cursor=opaque%2F%2B%3Fsnapshot',
    );
    const headers = new Headers(request?.[1]?.headers);
    expect(headers.get('authorization')).toBe('Bearer creator');
    expect(headers.get('accept')).toBe('text/event-stream');
    expect(headers.get('last-event-id')).toBe('header-cursor');
    expect(headers.has('x-cmaeg-owner')).toBe(false);
    expect(request?.[1]?.method).toBe('GET');
    expect(fetch).toHaveBeenCalledTimes(2);
  });

  it.each([
    ['unknown event', frame('delta')],
    ['cursor disagreement', frame('activity', page, 'wrong')],
    ['missing explicit cursor', `event: activity\ndata: ${JSON.stringify(page)}\n\n`],
    ['inherited cursor', `${frame('activity')}event: activity\ndata: ${JSON.stringify(page)}\n\n`],
    ['partial snapshot', frame('activity').slice(0, -2)],
    ['missing initial snapshot', ': keepalive\n\n'],
    [
      'invalid schema',
      frame('activity', { ...page, limit: '50' } as unknown as cmaeg.ActivityPage),
    ],
  ])('rejects %s without reconnecting', async (_name, body) => {
    const fetch = vi.fn<typeof globalThis.fetch>().mockResolvedValue(stream(body));
    const client = new Cmaeg({ baseUrl: 'https://gateway.example', fetch });
    await expect(async () => {
      for await (const event of client.streamChannelActivity('channel')) void event;
    }).rejects.toBeInstanceOf(ProtocolError);
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it('rejects invalid owner and header values locally and cancels observation', async () => {
    const fetch = vi.fn<typeof globalThis.fetch>();
    const client = new Cmaeg({ baseUrl: 'https://gateway.example', fetch });
    expect(() => client.getChannelActivity('channel', { owner: 'bad owner' })).toThrow(
      ValidationError,
    );
    await expect(
      client.streamChannelActivity('channel', { lastEventId: 'bad\nheader' }).next(),
    ).rejects.toBeInstanceOf(ValidationError);
    const controller = new AbortController();
    controller.abort();
    await expect(
      client.streamChannelActivity('channel', { signal: controller.signal }).next(),
    ).rejects.toBeInstanceOf(RequestAborted);
    expect(fetch).not.toHaveBeenCalled();
  });
});
