import { afterEach, describe, expect, it, vi } from 'vitest';
import { AgentConversations, type AgentConversationEvent } from '../src/agui/index.js';
import { resolveConfig, type ClientConfig } from '../src/config.js';
import {
  ApiError,
  ProtocolError,
  RequestAborted,
  TimeoutError,
  TransportError,
  ValidationError,
} from '../src/errors.js';

const agent = 'agt_stream';
const run = {
  threadId: agent,
  runId: 'client-correlation',
  messages: [{ role: 'user' as const, content: 'Hello' }],
};

function frame(type: string, cursor?: number, fields: Record<string, unknown> = {}): string {
  return `${cursor === undefined ? '' : `id: ${String(cursor)}\n`}data: ${JSON.stringify({ type, threadId: agent, runId: '1', ...fields })}\n\n`;
}

function sse(body: string): Response {
  return new Response(body, { headers: { 'content-type': 'text/event-stream; charset=utf-8' } });
}

function harness(config: ClientConfig = {}) {
  const fetch = vi.fn<typeof globalThis.fetch>();
  const requestIdProvider = vi.fn(() => 'stream-proof');
  const client = new AgentConversations(
    resolveConfig({ baseUrl: 'https://cma.example', fetch, requestIdProvider, ...config }),
  );
  const events: AgentConversationEvent[] = [];
  return {
    fetch,
    client,
    events,
    requestIdProvider,
    onEvent: (event: AgentConversationEvent) => {
      events.push(event);
    },
  };
}

afterEach(() => {
  vi.useRealTimers();
});

describe('Agent conversation projection', () => {
  it.each([null, undefined, [], 'not-an-object'])(
    'refuses malformed run input %j before transport',
    async (body) => {
      const h = harness();
      await expect(h.client.run(body as never, { onEvent: h.onEvent })).rejects.toBeInstanceOf(
        ValidationError,
      );
      expect(h.fetch).not.toHaveBeenCalled();
    },
  );

  it('delivers content-first live frames immediately and acknowledges complete durable projections', async () => {
    const h = harness();
    h.fetch.mockResolvedValueOnce(
      sse(
        frame('RUN_STARTED', 1) +
          frame('TEXT_MESSAGE_CONTENT', undefined, {
            'cma:live': true,
            messageId: 'live-1',
            delta: 'Hello',
          }) +
          frame('TEXT_MESSAGE_START', undefined, { messageId: 'msg_2', role: 'assistant' }) +
          frame('TEXT_MESSAGE_CONTENT', undefined, { messageId: 'msg_2', delta: 'Hello' }) +
          frame('TEXT_MESSAGE_END', 2, { messageId: 'msg_2' }) +
          frame('RUN_FINISHED', 3),
      ),
    );
    await h.client.run(run, { onEvent: h.onEvent });
    expect(h.events.map((event) => event.cursor)).toEqual([
      1,
      undefined,
      undefined,
      undefined,
      2,
      3,
    ]);
    expect(h.events[1]?.['cma:live']).toBe(true);
    expect(h.fetch).toHaveBeenCalledOnce();
    const [url, init] = h.fetch.mock.calls[0] ?? [];
    expect(url).toBe('https://cma.example/api/v1/agui/run');
    expect(init).toMatchObject({ method: 'POST', credentials: 'include', redirect: 'error' });
    expect(JSON.parse(String(init?.body))).toEqual(run);
    expect(new Headers(init?.headers).get('idempotency-key')).toBeNull();
  });

  it('keeps a continuation boundary cursorless and accepts a durable failed outcome', async () => {
    const h = harness();
    h.fetch.mockResolvedValueOnce(
      sse(frame('RUN_STARTED') + frame('RUN_ERROR', 4, { code: 'failed', message: 'Stopped' })),
    );
    await h.client.run(
      {
        threadId: agent,
        resume: [{ interruptId: 'approval-1', status: 'resolved', payload: { decision: 'deny' } }],
      },
      { onEvent: h.onEvent },
    );
    expect(h.events.map((event) => event.cursor)).toEqual([undefined, 4]);
    expect(h.events.at(-1)?.type).toBe('RUN_ERROR');
  });

  it('reads one bounded history when follow is false and preserves unknown event fields', async () => {
    const h = harness();
    h.fetch.mockResolvedValueOnce(
      sse(': keepalive\r\n\r\n' + frame('FUTURE_EVENT', 8, { extra: { preserved: true } })),
    );
    await h.client.replay(agent, { after: 7, follow: false, onEvent: h.onEvent });
    expect(h.fetch.mock.calls[0]?.[0]).toBe(
      'https://cma.example/api/v1/agui/events?thread_id=agt_stream&starting_after=7&follow=false',
    );
    expect(h.events[0]).toMatchObject({ cursor: 8, extra: { preserved: true } });
    expect(new Headers(h.fetch.mock.calls[0]?.[1]?.headers).get('last-event-id')).toBeNull();
  });

  it('finishes an approval resume with remaining requests at the committed head', async () => {
    const h = harness();
    const pending = { id: 'approval-2', reason: 'confirmation', message: 'Read instructions' };
    h.fetch.mockResolvedValueOnce(
      sse(
        frame('RUN_STARTED') +
          frame('STATE_DELTA', undefined, {
            delta: [{ op: 'remove', path: '/pendingApprovals/approval-1' }],
          }) +
          frame('STATE_DELTA', undefined, {
            delta: [{ op: 'add', path: '/pendingApprovals/approval-2', value: pending }],
          }) +
          frame('RUN_FINISHED', 9, { outcome: { type: 'interrupt', interrupts: [pending] } }),
      ),
    );
    await h.client.run(
      {
        threadId: agent,
        resume: [{ interruptId: 'approval-1', status: 'resolved', payload: { decision: 'allow' } }],
      },
      { onEvent: h.onEvent },
    );
    expect(h.events.map((event) => event.cursor)).toEqual([undefined, undefined, undefined, 9]);
    expect(h.events.at(-1)?.outcome).toEqual({ type: 'interrupt', interrupts: [pending] });
    expect(h.fetch).toHaveBeenCalledOnce();
  });

  it('does not expose an overlapping projection before rejecting its final cursor', async () => {
    const h = harness();
    h.fetch.mockResolvedValueOnce(
      sse(
        frame('TEXT_MESSAGE_START') +
          frame('TEXT_MESSAGE_CONTENT', undefined, { delta: 'Duplicate' }) +
          frame('TEXT_MESSAGE_END', 4),
      ),
    );
    await expect(
      h.client.replay(agent, { after: 4, follow: false, onEvent: h.onEvent }),
    ).rejects.toBeInstanceOf(ProtocolError);
    expect(h.events).toEqual([]);
  });

  it('discards incomplete projections and reconnects reads from the last acknowledged durable cursor', async () => {
    vi.useFakeTimers();
    const credential = vi
      .fn()
      .mockResolvedValueOnce('local-first')
      .mockResolvedValueOnce('local-second')
      .mockResolvedValueOnce('local-third');
    const h = harness({ credential, cookies: false });
    h.fetch.mockResolvedValueOnce(sse(frame('RUN_STARTED', 1) + frame('TEXT_MESSAGE_START')));
    h.fetch.mockResolvedValueOnce(
      sse(
        frame('TEXT_MESSAGE_START') +
          frame('TEXT_MESSAGE_CONTENT', undefined, { delta: 'Once' }) +
          frame('TEXT_MESSAGE_END', 2),
      ),
    );
    h.fetch.mockResolvedValueOnce(
      new Response(JSON.stringify({ status: 401, detail: 'Sign in', code: 'unauthorized' }), {
        status: 401,
      }),
    );
    const result = h.client.replay(agent, { onEvent: h.onEvent }).catch((error: unknown) => error);
    await vi.runAllTimersAsync();
    expect(await result).toBeInstanceOf(ApiError);
    expect(h.events.map((event) => event.type)).toEqual([
      'RUN_STARTED',
      'TEXT_MESSAGE_START',
      'TEXT_MESSAGE_CONTENT',
      'TEXT_MESSAGE_END',
    ]);
    expect(h.fetch.mock.calls.map(([url]) => String(url))).toEqual([
      expect.stringContaining('starting_after=0'),
      expect.stringContaining('starting_after=1'),
      expect.stringContaining('starting_after=2'),
    ]);
    expect(h.requestIdProvider).toHaveBeenCalledOnce();
    expect(credential).toHaveBeenCalledTimes(3);
    expect(
      h.fetch.mock.calls.map(([, init]) => new Headers(init?.headers).get('authorization')),
    ).toEqual(['Bearer local-first', 'Bearer local-second', 'Bearer local-third']);
  });

  it('does not let ephemeral traffic reset an empty reconnect limit', async () => {
    vi.useFakeTimers();
    const h = harness();
    h.fetch.mockImplementation(async () =>
      sse(frame('REASONING_MESSAGE_CONTENT', undefined, { 'cma:live': true, delta: 'Thinking' })),
    );
    const result = h.client
      .replay(agent, { maxReconnects: 1, onEvent: h.onEvent })
      .catch((error: unknown) => error);
    await vi.runAllTimersAsync();
    expect(await result).toBeInstanceOf(TransportError);
    expect(h.fetch).toHaveBeenCalledTimes(2);
  });

  it('preserves a named capacity refusal when the reconnect budget is exhausted', async () => {
    const h = harness();
    h.fetch.mockResolvedValueOnce(
      new Response(
        JSON.stringify({
          status: 429,
          code: 'relay_observer_capacity_exhausted',
          detail: 'Retry observation.',
        }),
        { status: 429, headers: { 'retry-after': '1' } },
      ),
    );
    await expect(
      h.client.replay(agent, { maxReconnects: 0, onEvent: h.onEvent }),
    ).rejects.toMatchObject({
      status: 429,
      code: 'relay_observer_capacity_exhausted',
      retryAfterMs: 1000,
    });
    expect(h.fetch).toHaveBeenCalledOnce();
  });

  it('honors capacity Retry-After and resumes from the same acknowledged cursor', async () => {
    vi.useFakeTimers();
    const h = harness();
    h.fetch.mockResolvedValueOnce(
      new Response(
        JSON.stringify({
          status: 429,
          code: 'relay_observer_capacity_exhausted',
          detail: 'Retry observation.',
        }),
        { status: 429, headers: { 'retry-after': '1' } },
      ),
    );
    h.fetch.mockResolvedValueOnce(sse(frame('RUN_STARTED', 8)));
    h.fetch.mockResolvedValueOnce(
      new Response(JSON.stringify({ status: 401, code: 'unauthorized', detail: 'Sign in' }), {
        status: 401,
      }),
    );
    const result = h.client
      .replay(agent, { after: 7, onEvent: h.onEvent })
      .catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(999);
    expect(h.fetch).toHaveBeenCalledOnce();
    await vi.advanceTimersByTimeAsync(1);
    expect(h.fetch).toHaveBeenCalledTimes(2);
    await vi.runAllTimersAsync();
    expect(await result).toMatchObject({ status: 401 });
    expect(h.events).toHaveLength(1);
    expect(h.fetch.mock.calls.map(([url]) => String(url))).toEqual([
      expect.stringContaining('starting_after=7'),
      expect.stringContaining('starting_after=7'),
      expect.stringContaining('starting_after=8'),
    ]);
  });

  it('never retries a POST after a disconnected run or a transport failure', async () => {
    for (const response of [sse(frame('RUN_STARTED', 1)), new Error('offline')]) {
      const h = harness();
      if (response instanceof Error) h.fetch.mockRejectedValueOnce(response);
      else h.fetch.mockResolvedValueOnce(response);
      await expect(h.client.run(run, { onEvent: h.onEvent })).rejects.toBeInstanceOf(
        TransportError,
      );
      expect(h.fetch).toHaveBeenCalledOnce();
    }
  });

  it('stops after a callback failure without acknowledging or retrying input', async () => {
    const h = harness();
    h.fetch.mockResolvedValueOnce(sse(frame('RUN_STARTED', 1)));
    await expect(
      h.client.run(run, {
        onEvent() {
          throw new Error('consumer failed');
        },
      }),
    ).rejects.toBeInstanceOf(ProtocolError);
    expect(h.fetch).toHaveBeenCalledOnce();
  });
});

describe('AG-UI transport safety', () => {
  it.each([
    frame('RUN_STARTED', 1, { threadId: 'agt_foreign' }),
    frame('', 1),
    frame('RUN_STARTED', 1, { runId: '' }),
    frame('RUN_STARTED', 1, { timestamp: 7 }),
    frame('REASONING_MESSAGE_CONTENT', 1, { 'cma:live': true }),
    frame('TEXT_MESSAGE_CONTENT', undefined, { 'cma:live': 'yes' }),
    'id: 01\ndata: {"type":"RUN_STARTED","threadId":"agt_stream","runId":"1"}\n\n',
    'id: 9007199254740992\ndata: {"type":"RUN_STARTED","threadId":"agt_stream","runId":"1"}\n\n',
    'event: WRONG\n' + frame('RUN_STARTED', 1),
    'data: not-json\n\n',
    'data: []\n\n',
  ])('rejects malformed, crossed, or cursor-forging SSE: %s', async (body) => {
    const h = harness();
    h.fetch.mockResolvedValueOnce(sse(body));
    await expect(
      h.client.replay(agent, { follow: false, onEvent: h.onEvent }),
    ).rejects.toBeInstanceOf(ProtocolError);
    expect(h.events).toEqual([]);
  });

  it('does not trust a payload cursor and does not deliver a partial event at EOF', async () => {
    const h = harness();
    h.fetch.mockResolvedValueOnce(
      sse(
        frame('REASONING_MESSAGE_CONTENT', undefined, { 'cma:live': true, cursor: 99 }) +
          'data: {"type":"RUN_STARTED"',
      ),
    );
    await h.client.replay(agent, { follow: false, onEvent: h.onEvent });
    expect(h.events).toHaveLength(1);
    expect(h.events[0]?.cursor).toBeUndefined();
  });

  it('handles a multibyte code point split across transport chunks', async () => {
    const h = harness();
    const bytes = new TextEncoder().encode(frame('FUTURE_EVENT', 1, { content: '\u00e9' }));
    const cut = bytes.indexOf(0xc3) + 1;
    h.fetch.mockResolvedValueOnce(
      new Response(
        new ReadableStream({
          start(controller) {
            controller.enqueue(bytes.slice(0, cut));
            controller.enqueue(bytes.slice(cut));
            controller.close();
          },
        }),
        { headers: { 'content-type': 'text/event-stream' } },
      ),
    );
    await h.client.replay(agent, { follow: false, onEvent: h.onEvent });
    expect(h.events[0]?.content).toBe('\u00e9');
  });

  it('rejects invalid UTF-8 and bounded frame, chunk, and projection overflows', async () => {
    const bodies = [
      new Uint8Array([0xff]),
      new TextEncoder().encode(frame('FUTURE_EVENT', 1, { text: 'x'.repeat(2 * 1024 * 1024) })),
      new Uint8Array(4 * 1024 * 1024 + 1),
      new TextEncoder().encode(frame('PREFIX').repeat(33)),
    ];
    for (const body of bodies) {
      const h = harness();
      h.fetch.mockResolvedValueOnce(
        new Response(body, { headers: { 'content-type': 'text/event-stream' } }),
      );
      await expect(
        h.client.replay(agent, { follow: false, onEvent: h.onEvent }),
      ).rejects.toBeInstanceOf(ProtocolError);
    }
  });

  it.each([201, 204])('requires exact HTTP 200, not successful status %i', async (status) => {
    const h = harness();
    h.fetch.mockResolvedValueOnce(
      new Response(null, { status, headers: { 'content-type': 'text/event-stream' } }),
    );
    await expect(h.client.run(run, { onEvent: h.onEvent })).rejects.toBeInstanceOf(ProtocolError);
  });

  it('preserves typed problem details and response observation', async () => {
    const observer = vi.fn();
    const h = harness({ responseObserver: observer });
    h.fetch.mockResolvedValueOnce(
      new Response(
        JSON.stringify({ status: 409, code: 'conflict', detail: 'Not ready', phase: 'restoring' }),
        { status: 409, headers: { 'x-request-id': 'server-proof' } },
      ),
    );
    await expect(h.client.run(run, { onEvent: h.onEvent })).rejects.toMatchObject({
      status: 409,
      code: 'conflict',
      requestId: 'server-proof',
      extensions: { phase: 'restoring' },
    });
    expect(observer).toHaveBeenCalledWith({ status: 409, requestId: 'server-proof' });
  });

  it.each([
    new Response('{}'),
    new Response(null, { headers: { 'content-type': 'text/event-stream' } }),
  ])('refuses the wrong content type or missing stream', async (response) => {
    const h = harness();
    h.fetch.mockResolvedValueOnce(response);
    await expect(h.client.run(run, { onEvent: h.onEvent })).rejects.toBeInstanceOf(ProtocolError);
  });

  it('cancels before credentials, during admission, and during body reads', async () => {
    const before = harness();
    const aborted = AbortSignal.abort();
    await expect(
      before.client.run(run, { signal: aborted, onEvent: before.onEvent }),
    ).rejects.toBeInstanceOf(RequestAborted);
    expect(before.fetch).not.toHaveBeenCalled();
    const pending = harness();
    const controller = new AbortController();
    pending.fetch.mockImplementation(async () => new Promise<Response>(() => undefined));
    const result = pending.client.run(run, { signal: controller.signal, onEvent: pending.onEvent });
    controller.abort();
    await expect(result).rejects.toBeInstanceOf(RequestAborted);
    const body = harness();
    const reading = new AbortController();
    const cancel = vi.fn();
    body.fetch.mockResolvedValueOnce(
      new Response(
        new ReadableStream({
          start(stream) {
            stream.enqueue(new TextEncoder().encode(frame('RUN_STARTED', 1)));
          },
          cancel,
        }),
        { headers: { 'content-type': 'text/event-stream' } },
      ),
    );
    await expect(
      body.client.run(run, {
        signal: reading.signal,
        onEvent() {
          reading.abort();
        },
      }),
    ).rejects.toBeInstanceOf(RequestAborted);
    expect(cancel).toHaveBeenCalledOnce();
  });

  it('times out a stalled handshake without limiting an established stream lifetime', async () => {
    vi.useFakeTimers();
    const h = harness();
    h.fetch.mockImplementationOnce(async () => new Promise<Response>(() => undefined));
    const timeout = h.client
      .run(run, { timeoutMs: 10, onEvent: h.onEvent })
      .catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(10);
    expect(await timeout).toBeInstanceOf(TimeoutError);
    let stream: ReadableStreamDefaultController<Uint8Array> | undefined;
    h.fetch.mockResolvedValueOnce(
      new Response(
        new ReadableStream({
          start(value) {
            stream = value;
          },
        }),
        { headers: { 'content-type': 'text/event-stream' } },
      ),
    );
    const active = h.client.run(run, { timeoutMs: 10, onEvent: h.onEvent });
    await vi.advanceTimersByTimeAsync(100);
    stream?.enqueue(new TextEncoder().encode(frame('RUN_FINISHED', 2)));
    stream?.close();
    await expect(active).resolves.toBeUndefined();
  });

  it('cancels reconnect backoff and rejects invalid options before transport', async () => {
    vi.useFakeTimers();
    const h = harness();
    h.fetch.mockResolvedValueOnce(sse(''));
    const stop = new AbortController();
    const result = h.client
      .replay(agent, { signal: stop.signal, onEvent: h.onEvent })
      .catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(0);
    stop.abort();
    expect(await result).toBeInstanceOf(RequestAborted);
    for (const options of [{ after: -1 }, { after: 0.5 }, { maxReconnects: 21 }]) {
      await expect(
        h.client.replay(agent, { ...options, onEvent: h.onEvent }),
      ).rejects.toBeInstanceOf(ValidationError);
    }
    await expect(h.client.run({ threadId: '' }, { onEvent: h.onEvent })).rejects.toBeInstanceOf(
      ValidationError,
    );
    await expect(h.client.run(run, { timeoutMs: 0, onEvent: h.onEvent })).rejects.toBeInstanceOf(
      ValidationError,
    );
  });
});
