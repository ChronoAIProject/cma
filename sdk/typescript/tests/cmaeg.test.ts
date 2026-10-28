import { createHmac } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import {
  Cmaeg,
  ApiError,
  ProtocolError,
  RequestAborted,
  TimeoutError,
  TransportError,
} from '../src/index.js';
import { signWebhook } from '../src/cmaeg/signing.js';
import { ChannelCreateRequestSchema, ChannelUpdateRequestSchema } from '../src/cmaeg/types.js';
import type { CloudEvent } from '../src/cmaeg/types.js';

const examples = JSON.parse(
  readFileSync(
    new URL('../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json', import.meta.url),
    'utf8',
  ),
) as Record<string, unknown[]>;
const sample = (name: string) => examples[name]?.[0];
const event = (): CloudEvent => ({
  specversion: '1.0',
  id: 'event-1',
  source: 'urn:fixture',
  type: 'example.event',
  data: { value: 'before' },
  traceid: 'trace',
});
const json = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), { status, headers: { 'content-type': 'application/json' } });

it('returns an empty native thread scan page without automatically fetching its continuation', async () => {
  const expected = examples.ConversationHistoryPage?.[2];
  const fetch = vi.fn<typeof globalThis.fetch>().mockResolvedValueOnce(json(expected));
  const client = new Cmaeg({ baseUrl: 'https://gateway.example', fetch });
  const page = await client.readEventHistory('original');
  expect(page).toEqual(expected);
  expect(page.messages).toEqual([]);
  expect(page.scanned_messages).toBe(25);
  expect(page.continuation.status).toBe('more_visible_rows');
  expect(fetch).toHaveBeenCalledOnce();
  expect(fetch.mock.calls[0]?.[0]).toBe('https://gateway.example/v1/events/original/history');
});

it('preserves native mixed post text and media omission without fetching media or continuation', async () => {
  const expected = examples.ConversationHistoryPage?.[3];
  const fetch = vi.fn<typeof globalThis.fetch>().mockResolvedValueOnce(json(expected));
  const client = new Cmaeg({ baseUrl: 'https://gateway.example', fetch });
  const page = await client.readEventHistory('original');
  expect(page).toEqual(expected);
  expect(page.messages[0]?.text).toBe('Earlier postText before and after omitted media');
  expect(page.messages[0]?.omitted_content).toEqual(['media']);
  expect(page.messages[0]?.attachments).toEqual([]);
  expect(fetch).toHaveBeenCalledOnce();
});

describe('independent CMAEG client', () => {
  it('emits all management and tool methods with opaque IDs, complete bodies and exact versions', async () => {
    const fetch = vi.fn<typeof globalThis.fetch>();
    const client = new Cmaeg({
      baseUrl: 'https://gateway.example',
      credential: () => 'fixture-custody',
      fetch,
    });
    const id = 'opaque/+?%value';
    const segment = encodeURIComponent(id);
    const create = ChannelCreateRequestSchema(sample('ChannelCreateRequest'));
    const update = ChannelUpdateRequestSchema(sample('ChannelUpdateRequest'));
    const message = { text: 'hello', blocks: [] };
    const cases = [
      {
        run: () => client.createChannel(create, { idempotencyKey: 'retry-same-request' }),
        method: 'POST',
        path: '/v1/channels',
        body: create,
        response: sample('ChannelResponse'),
        status: 201,
      },
      {
        run: () => client.listChannels({ after: 'cursor/one' }),
        method: 'GET',
        path: '/v1/channels?after=cursor%2Fone',
        response: sample('ChannelPage'),
      },
      {
        run: () => client.getChannel(id),
        method: 'GET',
        path: `/v1/channels/${segment}`,
        response: sample('ChannelResponse'),
      },
      {
        run: () => client.recoverChannel(id),
        method: 'GET',
        path: `/v1/channel-records/${segment}`,
        response: sample('ChannelResponse'),
      },
      {
        run: () => client.updateChannel(id, update),
        method: 'PUT',
        path: `/v1/channels/${segment}`,
        body: update,
        response: sample('ChannelResponse'),
      },
      {
        run: () => client.deleteChannel(id, 42),
        method: 'DELETE',
        path: `/v1/channels/${segment}?expected_version=42`,
        status: 204,
      },
      {
        run: () => client.testChannel(id),
        method: 'POST',
        path: `/v1/channels/${segment}/test`,
        response: sample('TestChannelResponse'),
      },
      {
        run: () => client.rekeyChannel(id),
        method: 'POST',
        path: `/v1/channels/${segment}/rekey`,
        response: sample('RekeyResponse'),
      },
      {
        run: () => client.replyToEvent(id, message),
        method: 'POST',
        path: `/v1/events/${segment}/replies`,
        body: message,
        response: sample('IntentReceipt'),
      },
      {
        run: () => client.updateMessage(id, message),
        method: 'POST',
        path: `/v1/messages/${segment}/update`,
        body: message,
        response: sample('IntentReceipt'),
      },
      {
        run: () => client.sendTyping(id),
        method: 'POST',
        path: `/v1/events/${segment}/typing`,
        response: sample('TypingReceipt'),
      },
      {
        run: () => client.readEventHistory(id),
        method: 'GET',
        path: `/v1/events/${segment}/history`,
        response: sample('ConversationHistoryPage'),
      },
      {
        run: () => client.readEventHistory(id, 'opaque/+?cursor'),
        method: 'GET',
        path: `/v1/events/${segment}/history?cursor=opaque%2F%2B%3Fcursor`,
        response: sample('ConversationHistoryPage'),
      },
      {
        run: () => client.readEventContext(id),
        method: 'GET',
        path: `/v1/events/${segment}/context`,
        response: sample('ContextResponse'),
      },
      {
        run: () => client.readEventContext(id, { includeThreadHistory: true }),
        method: 'GET',
        path: `/v1/events/${segment}/context?include_thread_history=true`,
        response: sample('ContextResponse'),
      },
    ];
    for (const scenario of cases) {
      fetch.mockResolvedValueOnce(
        scenario.status === 204
          ? new Response(null, { status: 204 })
          : json(scenario.response, scenario.status),
      );
      await scenario.run();
      const call = fetch.mock.calls.at(-1);
      expect(call?.[0]).toBe(`https://gateway.example${scenario.path}`);
      expect(call?.[1]?.method).toBe(scenario.method);
      expect(call?.[1]?.redirect).toBe('manual');
      expect(call?.[1]?.body).toBe(
        scenario.body === undefined ? undefined : JSON.stringify(scenario.body),
      );
      expect(new Headers(call?.[1]?.headers).get('authorization')).toBe('Bearer fixture-custody');
    }
    expect(new Headers(fetch.mock.calls[0]?.[1]?.headers).get('idempotency-key')).toBe(
      'retry-same-request',
    );
  });

  it('signs the exact UTF-8 secret string and snapshots event bytes before asynchronous signing', async () => {
    const secret = btoa('local signing fixture');
    const headers = await signWebhook(secret, 'delivery', '123', '{"é":1}');
    expect(headers['webhook-signature']).toBe(
      `v1,${createHmac('sha256', secret).update('delivery.123.{"é":1}').digest('base64')}`,
    );
    const fetch = vi
      .fn<typeof globalThis.fetch>()
      .mockResolvedValue(
        json({ event_id: 'event-1', conversation_id: 'conversation', status: 'accepted' }, 202),
      );
    const credential = vi.fn(() => 'unused-creator-custody');
    const client = new Cmaeg({ baseUrl: 'https://gateway.example', credential, fetch });
    const body = event();
    const posted = client.postEvent('channel', body, {
      webhook: { secret, id: 'delivery', timestamp: '123' },
    });
    (body.data as { value: string }).value = 'after';
    await posted;
    const init = fetch.mock.calls[0]?.[1];
    const sent = String(init?.body);
    expect(JSON.parse(sent)).toMatchObject({ data: { value: 'before' }, traceid: 'trace' });
    expect(new Headers(init?.headers).get('content-type')).toBe('application/cloudevents+json');
    expect(new Headers(init?.headers).get('webhook-signature')).toBe(
      `v1,${createHmac('sha256', secret).update(`delivery.123.${sent}`).digest('base64')}`,
    );
    expect(credential).not.toHaveBeenCalled();
    expect(new Headers(init?.headers).has('authorization')).toBe(false);
  });

  it('preserves typed problem codes and never retries an uncertain mutation', async () => {
    const fetch = vi
      .fn<typeof globalThis.fetch>()
      .mockResolvedValueOnce(
        json(
          { title: 'Conflict', status: 409, code: 'conflict', detail: 'stale record version' },
          409,
        ),
      );
    const client = new Cmaeg({ baseUrl: 'https://gateway.example', fetch });
    const deleted = client.deleteChannel('channel', 1);
    await expect(deleted).rejects.toBeInstanceOf(ApiError);
    await expect(deleted).rejects.toMatchObject({
      code: 'conflict',
      status: 409,
    });
    fetch.mockRejectedValueOnce(new TypeError('lost connection'));
    await expect(client.replyToEvent('event', { text: 'hello' })).rejects.toBeInstanceOf(
      TransportError,
    );
    expect(fetch).toHaveBeenCalledTimes(2);
  });

  it('waits through cold channel startup and completion on one test request', async () => {
    vi.useFakeTimers();
    try {
      const fetch = vi
        .fn<typeof globalThis.fetch>()
        .mockImplementation(
          () =>
            new Promise((resolve) =>
              setTimeout(() => resolve(json(sample('TestChannelResponse'))), 120_000),
            ),
        );
      const client = new Cmaeg({ baseUrl: 'https://gateway.example', fetch });
      const result = client.testChannel('cold-channel');
      const expected = expect(result).resolves.toEqual(sample('TestChannelResponse'));
      await vi.advanceTimersByTimeAsync(120_000);
      await expected;
      expect(fetch).toHaveBeenCalledOnce();
      expect(fetch.mock.calls[0]?.[1]?.signal?.aborted).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it.each([undefined, 25])(
    'bounds a single test request with its default or caller deadline: %s',
    async (timeoutMs) => {
      vi.useFakeTimers();
      try {
        const fetch = vi
          .fn<typeof globalThis.fetch>()
          .mockImplementation(() => new Promise(() => {}));
        const client = new Cmaeg({ baseUrl: 'https://gateway.example', fetch });
        const result = client.testChannel('cold-channel', { timeoutMs });
        const expected = expect(result).rejects.toBeInstanceOf(TimeoutError);
        const deadline = timeoutMs ?? 125_000;
        await vi.advanceTimersByTimeAsync(deadline - 1);
        expect(fetch).toHaveBeenCalledOnce();
        expect(fetch.mock.calls[0]?.[1]?.signal?.aborted).toBe(false);
        await vi.advanceTimersByTimeAsync(1);
        await expected;
        expect(fetch.mock.calls[0]?.[1]?.signal?.aborted).toBe(true);
        expect(fetch).toHaveBeenCalledOnce();
      } finally {
        vi.useRealTimers();
      }
    },
  );

  it('preserves caller cancellation without retrying a channel test', async () => {
    const fetch = vi.fn<typeof globalThis.fetch>().mockImplementation(() => new Promise(() => {}));
    const controller = new AbortController();
    const client = new Cmaeg({ baseUrl: 'https://gateway.example', fetch });
    const result = client.testChannel('cold-channel', { signal: controller.signal });
    const expected = expect(result).rejects.toBeInstanceOf(RequestAborted);
    await vi.waitFor(() => expect(fetch).toHaveBeenCalledOnce());
    controller.abort();
    await expected;
    expect(fetch.mock.calls[0]?.[1]?.signal?.aborted).toBe(true);
    expect(fetch).toHaveBeenCalledOnce();
  });
});

function frame(name: string, data: unknown) {
  return `event: ${name}\ndata: ${JSON.stringify(data)}\n\n`;
}
const identity = { event_id: 'event-1', conversation_id: 'conversation' };
const completed = frame('cmaeg.completed', {
  ...identity,
  text: 'done',
  message: null,
  error: null,
});
async function collect(client: Cmaeg) {
  const result = [];
  for await (const value of client.streamEvent('channel', event())) result.push(value);
  return result;
}

describe('custom event streams', () => {
  it('decodes fragmented UTF-8, preserves event identity and cancels observation after completion', async () => {
    const bytes = new TextEncoder().encode(
      ': keepalive\n\n' + frame('cmaeg.text.delta', { ...identity, text: 'hello 🌍' }) + completed,
    );
    const cancel = vi.fn();
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        for (let offset = 0; offset < bytes.length; offset += 7)
          controller.enqueue(bytes.slice(offset, offset + 7));
      },
      cancel,
    });
    const fetch = vi
      .fn<typeof globalThis.fetch>()
      .mockResolvedValue(new Response(body, { headers: { 'content-type': 'text/event-stream' } }));
    const client = new Cmaeg({ baseUrl: 'https://gateway.example', fetch });
    const events = await collect(client);
    expect(events.map((value) => value.event)).toEqual(['cmaeg.text.delta', 'cmaeg.completed']);
    expect(events[0]?.data).toMatchObject({ text: 'hello 🌍' });
    expect(cancel).toHaveBeenCalledOnce();
    expect(fetch).toHaveBeenCalledOnce();
  });

  it.each([
    frame('cmaeg.failed', { error: { code: 'internal', detail: 'sync_backpressure' } }),
    frame('cmaeg.failed', {
      ...identity,
      text: null,
      message: null,
      error: {
        type: 'about:blank',
        title: 'Unavailable',
        status: 503,
        code: 'provider_unavailable',
      },
    }),
  ])('retains documented terminal failures without reconnecting', async (body) => {
    const fetch = vi
      .fn<typeof globalThis.fetch>()
      .mockResolvedValue(new Response(body, { headers: { 'content-type': 'text/event-stream' } }));
    const values = await collect(new Cmaeg({ baseUrl: 'https://gateway.example', fetch }));
    expect(values[0]?.event).toBe('cmaeg.failed');
    expect(fetch).toHaveBeenCalledOnce();
  });

  it.each([
    frame('cmaeg.text.delta', { ...identity, event_id: 'another', text: 'wrong' }) + completed,
    frame('cmaeg.text.delta', { ...identity, text: 'é'.repeat(600_000) }) + completed,
    `id: replay-is-unsupported\n${completed}`,
  ])('refuses invalid stream identity, frame size or replay envelope', async (body) => {
    const fetch = vi
      .fn<typeof globalThis.fetch>()
      .mockResolvedValue(new Response(body, { headers: { 'content-type': 'text/event-stream' } }));
    await expect(
      collect(new Cmaeg({ baseUrl: 'https://gateway.example', fetch })),
    ).rejects.toBeInstanceOf(ProtocolError);
    expect(fetch).toHaveBeenCalledOnce();
  });

  it('reports truncated streams without silently reconnecting', async () => {
    const fetch = vi.fn<typeof globalThis.fetch>().mockResolvedValue(
      new Response(frame('cmaeg.text.delta', { ...identity, text: 'partial' }), {
        headers: { 'content-type': 'text/event-stream' },
      }),
    );
    await expect(
      collect(new Cmaeg({ baseUrl: 'https://gateway.example', fetch })),
    ).rejects.toBeInstanceOf(TransportError);
    expect(fetch).toHaveBeenCalledOnce();
  });

  it('keeps the deadline active while awaiting stream body bytes', async () => {
    const fetch = vi
      .fn<typeof globalThis.fetch>()
      .mockResolvedValue(
        new Response(new ReadableStream(), { headers: { 'content-type': 'text/event-stream' } }),
      );
    await expect(
      collect(new Cmaeg({ baseUrl: 'https://gateway.example', requestTimeoutMs: 20, fetch })),
    ).rejects.toBeInstanceOf(TimeoutError);
  });
});

describe('native relay media', () => {
  it('preserves explicit native attachments in the public reply request', async () => {
    const fetch = vi.fn<typeof globalThis.fetch>().mockResolvedValue(json(sample('IntentReceipt')));
    const client = new Cmaeg({
      baseUrl: 'https://gateway.example',
      credential: () => 'fixture-custody',
      fetch,
    });
    const message = {
      text: 'Report',
      native_attachments: [
        {
          kind: 'file' as const,
          source: { type: 'url' as const, url: 'https://files.example/report.pdf' },
          filename: 'report.pdf',
          mime_type: 'application/pdf',
          caption: 'Final report',
        },
      ],
    };
    await client.replyToEvent('opaque-event-reference', message);
    expect(JSON.parse(String(fetch.mock.calls[0]?.[1]?.body))).toEqual(message);
  });
});

it('uses scoped thread references, explicit stable keys and exact lifecycle wire methods', async () => {
  const fetch = vi.fn<typeof globalThis.fetch>();
  const client = new Cmaeg({
    baseUrl: 'https://gateway.example',
    credential: () => 'fixture',
    fetch,
  });
  const eventRef = 'opaque/+?%event';
  fetch.mockResolvedValueOnce(
    json({ threads: [], truncated: false, scope: 'public_children_of_original_parent' }),
  );
  await client.listEventThreads(eventRef, 'archived');
  expect(fetch.mock.calls[0]?.[0]).toBe(
    `https://gateway.example/v1/events/${encodeURIComponent(eventRef)}/threads?view=archived`,
  );
  const create = { name: 'Review', auto_archive_duration: 1440 };
  fetch.mockResolvedValueOnce(
    json({ outcome: { status: 'unknown', reason: 'thread_write_unconfirmed' } }),
  );
  const result = await client.createEventThread(eventRef, 'create-stable', create);
  expect(result.outcome.status).toBe('unknown');
  expect(fetch).toHaveBeenCalledTimes(2);
  const creation = fetch.mock.calls[1]?.[1];
  expect(creation?.method).toBe('POST');
  expect(new Headers(creation?.headers).get('Idempotency-Key')).toBe('create-stable');
  expect(JSON.parse(creation?.body as string)).toEqual(create);
  const request = {
    target: { type: 'thread' as const, thread_ref: 'cmaeg1.tr.original' },
    action: 'leave' as const,
  };
  fetch.mockResolvedValueOnce(json({ outcome: { status: 'confirmed' } }));
  await client.manageThread('leave-stable', request);
  expect(fetch.mock.calls[2]?.[0]).toBe('https://gateway.example/v1/thread-operations');
  expect(JSON.parse(fetch.mock.calls[2]?.[1]?.body as string)).toEqual(request);
  const rename = { ...request, action: 'rename' as const, name: 'Incident triage' };
  fetch.mockResolvedValueOnce(
    json({ outcome: { status: 'unknown', reason: 'thread_write_unconfirmed' } }),
  );
  const renamed = await client.manageThread('rename-stable', rename);
  expect(renamed.outcome.status).toBe('unknown');
  expect(fetch).toHaveBeenCalledTimes(4);
  expect(fetch.mock.calls[3]?.[0]).toBe('https://gateway.example/v1/thread-operations');
  expect(new Headers(fetch.mock.calls[3]?.[1]?.headers).get('Idempotency-Key')).toBe(
    'rename-stable',
  );
  expect(JSON.parse(fetch.mock.calls[3]?.[1]?.body as string)).toEqual(rename);
});
