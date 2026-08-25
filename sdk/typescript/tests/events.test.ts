import { describe, expect, it, vi } from 'vitest';
import type { EventSourceMessage } from 'eventsource-parser';
import { resolveConfig, type ResponseMetadata } from '../src/config.js';
import {
  ApiError,
  CapabilityRefused,
  RequestAborted,
  TimeoutError,
  TransportError,
  ValidationError,
} from '../src/errors.js';
import {
  KNOWN_SESSION_EVENT_TYPES,
  eventFromMessage,
  streamEvents,
  type SessionEvent,
} from '../src/events.js';

/** @vitest-environment node */

const SESSION = 'sesn_fixture';

function message(event: unknown, seq = 1, sessionId = SESSION): EventSourceMessage {
  return {
    event: undefined,
    id: String(seq),
    data: JSON.stringify({ seq, session_id: sessionId, event }),
  };
}

function sse(
  events: readonly { readonly seq: number; readonly event: unknown }[],
  sessionId = SESSION,
  requestId?: string,
): Response {
  const body = events
    .map(({ seq, event }) => {
      const data = JSON.stringify({ seq, session_id: sessionId, event });
      return `id: ${String(seq)}\ndata: ${data}\n\n`;
    })
    .join('');
  const response = new Response(body, {
    status: 200,
    headers: { 'Content-Type': 'text/event-stream; charset=utf-8' },
  });
  if (requestId !== undefined) {
    response.headers.set('x-request-id', requestId);
  }
  return response;
}

async function first(events: AsyncIterable<SessionEvent>): Promise<SessionEvent> {
  for await (const event of events) {
    return event;
  }
  throw new Error('the stream ended without an event');
}

async function take(events: AsyncIterable<SessionEvent>, count: number): Promise<SessionEvent[]> {
  const received: SessionEvent[] = [];
  for await (const event of events) {
    received.push(event);
    if (received.length === count) {
      break;
    }
  }
  return received;
}

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  const promise = new Promise<T>((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}

const nextTurn = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

describe('core event envelopes', () => {
  const known: readonly Record<string, unknown>[] = [
    { type: 'session_created' },
    { type: 'progress', stage: 'runtime_created' },
    {
      type: 'status_changed',
      status: 'running',
      stop_reason: null,
      detail: 'ready',
      capability: null,
    },
    { type: 'turn_started' },
    { type: 'turn_completed' },
    { type: 'terminal_attached' },
    { type: 'terminal_detached' },
    { type: 'terminal_resized', columns: 132, rows: 44 },
    { type: 'conversation_opened', conversation_id: 'conv_1' },
    { type: 'message_committed', content: '' },
    { type: 'owner_message', content: 'hello' },
    {
      type: 'tool_call_started',
      call_id: 'call_1',
      title: 'Read file',
      kind: 'read',
      arguments: { text: '{}', truncated: false },
    },
    {
      type: 'tool_call_updated',
      call_id: 'call_1',
      status: 'completed',
      result: { text: 'done' },
    },
    {
      type: 'usage_reported',
      input_tokens: 10,
      output_tokens: 3,
      context_used: 13,
      context_window: null,
    },
    { type: 'context_compacted', was: 200, now: 90, window: 300 },
    { type: 'mode_changed', mode: 'plan' },
    { type: 'plan_set', entries: [{ title: 'Ship', status: 'pending' }] },
    {
      type: 'config_options_set',
      options: [{ id: 'model', name: 'Model', kind: 'select' }],
    },
    {
      type: 'available_commands_set',
      commands: [{ name: 'review', description: 'Review changes' }],
    },
    { type: 'conversation_gap', frames: 2 },
    {
      type: 'sign_in_required',
      url: 'https://auth.example.test/device',
      code: null,
      message: 'Sign in',
    },
    { type: 'approval_requested', request_id: 'req_1', action: null, detail: 'write' },
    { type: 'approval_resolved', request_id: 'req_1', allowed: false },
    {
      type: 'plane_changed',
      from: 'terminal',
      to: 'conversation',
      snapshot_at: '2026-08-24T08:00:00Z',
    },
    {
      type: 'memory_pressure_opened',
      used_bytes: 100,
      limit_bytes: 200,
      occupancy_permille: 500,
    },
    {
      type: 'memory_pressure_closed',
      peak_bytes: 180,
      limit_bytes: 200,
      peak_permille: 900,
      duration_seconds: 3,
    },
    {
      type: 'runtime_lost',
      runtime_id: 'runtime_1',
      cause: 'out_of_memory',
    },
  ];

  it('accepts every known event type without changing its fields', () => {
    expect(known.map((event) => event.type)).toEqual(KNOWN_SESSION_EVENT_TYPES);

    for (const [index, event] of known.entries()) {
      expect(eventFromMessage(message(event, index + 1), SESSION)).toEqual({
        seq: index + 1,
        sessionId: SESSION,
        event,
      });
    }
  });

  it('preserves a newer event kind losslessly', () => {
    const raw = { type: 'future_event', nested: { value: 7 } };

    expect(eventFromMessage(message(raw), SESSION)).toEqual({
      seq: 1,
      sessionId: SESSION,
      event: { type: 'unknown', eventType: 'future_event', raw },
    });
  });

  it.each([
    [{ type: 'progress', stage: 'invented' }, /progress stage/i],
    [{ type: 'status_changed', status: 'ready', detail: 3 }, /detail/i],
    [{ type: 'terminal_resized', columns: 0, rows: 24 }, /columns/i],
    [{ type: 'conversation_opened', conversation_id: '' }, /conversation_id/i],
    [{ type: 'message_committed', content: 9 }, /content/i],
    [
      { type: 'tool_call_started', call_id: 'c', title: '', kind: 'read', arguments: [] },
      /arguments/i,
    ],
    [
      {
        type: 'tool_call_updated',
        call_id: 'c',
        status: 'done',
        result: { text: 'ok', truncated: 'no' },
      },
      /truncated/i,
    ],
    [
      { type: 'usage_reported', input_tokens: -1, output_tokens: 0, context_used: 0 },
      /input_tokens/i,
    ],
    [{ type: 'context_compacted', was: 1, now: 0, window: 0.5 }, /window/i],
    [{ type: 'mode_changed', mode: '' }, /mode/i],
    [{ type: 'plan_set', entries: 'none' }, /entries/i],
    [{ type: 'plan_set', entries: [{ title: 3, status: 'pending' }] }, /title/i],
    [{ type: 'config_options_set', options: [{ id: '', name: 'Model', kind: 'select' }] }, /id/i],
    [
      { type: 'available_commands_set', commands: [{ name: 'go', description: 3 }] },
      /description/i,
    ],
    [{ type: 'conversation_gap', frames: -1 }, /frames/i],
    [{ type: 'sign_in_required', url: '', code: null }, /url/i],
    [{ type: 'sign_in_required', url: 'https://x', code: 4 }, /code/i],
    [{ type: 'approval_requested', request_id: '', action: null }, /request_id/i],
    [{ type: 'approval_resolved', request_id: 'r', allowed: 'yes' }, /allowed/i],
    [
      {
        type: 'plane_changed',
        from: 'terminal',
        to: 'future',
        snapshot_at: 'now',
      },
      /to/i,
    ],
    [
      {
        type: 'memory_pressure_opened',
        used_bytes: 1,
        limit_bytes: 2,
        occupancy_permille: -1,
      },
      /occupancy/i,
    ],
    [
      {
        type: 'memory_pressure_closed',
        peak_bytes: 1,
        limit_bytes: 2,
        peak_permille: 500,
        duration_seconds: -1,
      },
      /duration/i,
    ],
    [{ type: 'runtime_lost', runtime_id: 'r', cause: 'mystery' }, /cause/i],
  ])('reports a malformed known event %#', (event, expected) => {
    expect(() => eventFromMessage(message(event), SESSION)).toThrow(expected);
  });

  it('rejects malformed framing and envelopes', () => {
    expect(() =>
      eventFromMessage({ ...message({ type: 'session_created' }), event: 'custom' }, SESSION),
    ).toThrow(/named event/i);
    expect(() =>
      eventFromMessage({ ...message({ type: 'session_created' }), id: undefined }, SESSION),
    ).toThrow(/positive sequence/i);
    expect(() =>
      eventFromMessage({ ...message({ type: 'session_created' }), id: '0' }, SESSION),
    ).toThrow(/positive sequence/i);
    expect(() =>
      eventFromMessage(
        { ...message({ type: 'session_created' }), id: '9007199254740992' },
        SESSION,
      ),
    ).toThrow(/safe integer/i);
    expect(() => eventFromMessage({ event: undefined, id: '1', data: '{' }, SESSION)).toThrow(
      /valid JSON/i,
    );
    expect(() => eventFromMessage({ event: undefined, id: '1', data: '[]' }, SESSION)).toThrow(
      /frame was not an object/i,
    );
    expect(() =>
      eventFromMessage(
        { ...message({ type: 'session_created' }), data: JSON.stringify({ seq: 2 }) },
        SESSION,
      ),
    ).toThrow(/did not match/i);
    expect(() =>
      eventFromMessage(message({ type: 'session_created' }, 1, 'other'), SESSION),
    ).toThrow(/arrived on stream/i);
    expect(() => eventFromMessage(message(null), SESSION)).toThrow(/event payload/i);
    expect(() => eventFromMessage(message({ type: '' }), SESSION)).toThrow(/type.*empty/i);
  });

  it('enforces the event-frame byte boundary', () => {
    const oversized = { type: 'future', payload: 'x'.repeat(2 * 1024 * 1024) };
    expect(() => eventFromMessage(message(oversized), SESSION)).toThrow(/byte boundary/i);
  });
});

describe('the reconnecting event stream', () => {
  it('opens with fetch and yields a strictly validated frame', async () => {
    const fetch = vi.fn(() =>
      Promise.resolve(sse([{ seq: 1, event: { type: 'session_created' } }])),
    );
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    await expect(first(streamEvents(config, SESSION))).resolves.toMatchObject({
      seq: 1,
      sessionId: SESSION,
      event: { type: 'session_created' },
    });
    const [url, init] = fetch.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe('/api/v1/sessions/sesn_fixture/events');
    expect(init).toMatchObject({ method: 'GET', credentials: 'include', redirect: 'error' });
    expect(new Headers(init.headers).get('accept')).toBe('text/event-stream');
  });

  it('reconnects from the last cursor, refreshes credentials, and deduplicates overlap', async () => {
    const responses = [
      sse([{ seq: 8, event: { type: 'session_created' } }], 'sesn/fixture', 'response-events-one'),
      sse(
        [
          { seq: 8, event: { type: 'session_created' } },
          { seq: 9, event: { type: 'turn_started' } },
        ],
        'sesn/fixture',
        'response-events-two',
      ),
    ];
    const calls: { readonly url: string; readonly init: RequestInit }[] = [];
    const fetch = vi.fn((url: string | URL | Request, init?: RequestInit) => {
      calls.push({ url: String(url), init: init ?? {} });
      const response = responses.shift();
      if (response === undefined) {
        return Promise.reject(new Error('unexpected reconnect'));
      }
      return Promise.resolve(response);
    });
    let credential = 0;
    const delays: number[] = [];
    const requestIdProvider = vi.fn(() => 'caller-event-stream-488');
    const observed: ResponseMetadata[] = [];
    const config = resolveConfig({
      cookies: false,
      credential: () => `token-${String(++credential)}`,
      fetch: fetch as unknown as typeof globalThis.fetch,
      requestIdProvider,
      responseObserver: (metadata) => {
        observed.push(metadata);
      },
    });

    const received = await take(
      streamEvents(config, 'sesn/fixture', {
        startingAfter: 7,
        initialBackoffMs: 10,
        maxBackoffMs: 100,
        jitterRatio: 0,
        sleep: (milliseconds) => {
          delays.push(milliseconds);
          return Promise.resolve();
        },
      }),
      2,
    );

    expect(received.map(({ seq }) => seq)).toEqual([8, 9]);
    expect(calls.map(({ url }) => url)).toEqual([
      '/api/v1/sessions/sesn%2Ffixture/events?after=7',
      '/api/v1/sessions/sesn%2Ffixture/events?after=8',
    ]);
    expect(calls.map(({ init }) => new Headers(init.headers).get('last-event-id'))).toEqual([
      '7',
      '8',
    ]);
    expect(calls.map(({ init }) => new Headers(init.headers).get('authorization'))).toEqual([
      'Bearer token-1',
      'Bearer token-2',
    ]);
    expect(calls.map(({ init }) => new Headers(init.headers).get('x-request-id'))).toEqual([
      'caller-event-stream-488',
      'caller-event-stream-488',
    ]);
    expect(requestIdProvider).toHaveBeenCalledOnce();
    expect(observed).toEqual([
      { status: 200, requestId: 'response-events-one' },
      { status: 200, requestId: 'response-events-two' },
    ]);
    expect(calls.every(({ init }) => init.credentials === 'omit')).toBe(true);
    expect(delays).toEqual([10]);
  });

  it('backs off exponentially across retryable transport failures', async () => {
    const answers: (Response | Error | string)[] = [
      new Error('offline'),
      'still offline',
      sse([{ seq: 1, event: { type: 'turn_started' } }]),
    ];
    const fetch = vi.fn(() => {
      const answer = answers.shift();
      return answer instanceof Response
        ? Promise.resolve(answer)
        : Promise.reject(answer ?? new Error('unexpected call'));
    });
    const delays: number[] = [];
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    await expect(
      first(
        streamEvents(config, SESSION, {
          initialBackoffMs: 10,
          maxBackoffMs: 100,
          jitterRatio: 0,
          sleep: (milliseconds) => {
            delays.push(milliseconds);
            return Promise.resolve();
          },
        }),
      ),
    ).resolves.toMatchObject({ seq: 1 });
    expect(delays).toEqual([10, 20]);
    expect(fetch).toHaveBeenCalledTimes(3);
  });

  it('retries a retryable HTTP problem and preserves a permanent refusal', async () => {
    const retrying = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ status: 503, detail: 'unavailable', code: 'unavailable' }), {
          status: 503,
        }),
      )
      .mockResolvedValueOnce(sse([{ seq: 1, event: { type: 'turn_started' } }]));
    const retryConfig = resolveConfig({
      fetch: retrying as unknown as typeof globalThis.fetch,
    });
    await expect(
      first(
        streamEvents(retryConfig, SESSION, {
          jitterRatio: 0,
          sleep: () => Promise.resolve(),
        }),
      ),
    ).resolves.toMatchObject({ seq: 1 });

    const refusing = vi.fn(() =>
      Promise.resolve(
        new Response(
          JSON.stringify({
            status: 422,
            detail: 'capability not supported: core_events',
            code: 'capability_unsupported',
            capability: 'core_events',
          }),
          { status: 422, headers: { 'x-request-id': 'response-event-refusal' } },
        ),
      ),
    );
    const refusal = await first(
      streamEvents(
        resolveConfig({ fetch: refusing as unknown as typeof globalThis.fetch }),
        SESSION,
      ),
    ).catch((cause: unknown) => cause);
    expect(refusal).toBeInstanceOf(CapabilityRefused);
    expect((refusal as CapabilityRefused).requestId).toBe('response-event-refusal');
    expect(refusing).toHaveBeenCalledOnce();
  });

  it('reads a typed refusal before closing the failed handshake', async () => {
    const refusing = vi.fn((_url: string | URL | Request, init?: RequestInit) => {
      const encoded = new TextEncoder().encode(
        JSON.stringify({
          status: 422,
          detail: 'capability not supported: core_events',
          code: 'capability_unsupported',
          capability: 'core_events',
        }),
      );
      const body = new ReadableStream<Uint8Array>({
        start(controller) {
          init?.signal?.addEventListener(
            'abort',
            () => controller.error(new DOMException('aborted', 'AbortError')),
            { once: true },
          );
          queueMicrotask(() => {
            controller.enqueue(encoded);
            controller.close();
          });
        },
      });
      return Promise.resolve(new Response(body, { status: 422 }));
    });

    const failure = await first(
      streamEvents(
        resolveConfig({ fetch: refusing as unknown as typeof globalThis.fetch }),
        SESSION,
        { maxEmptyConnections: 1 },
      ),
    ).catch((cause: unknown) => cause);

    expect(failure).toBeInstanceOf(CapabilityRefused);
    expect((failure as CapabilityRefused).capability).toBe('core_events');
  });

  it('stops after repeated empty connections', async () => {
    const fetch = vi.fn(() => Promise.resolve(sse([])));
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    await expect(
      first(
        streamEvents(config, SESSION, {
          maxEmptyConnections: 2,
          jitterRatio: 0,
          sleep: () => Promise.resolve(),
        }),
      ),
    ).rejects.toBeInstanceOf(TransportError);
    expect(fetch).toHaveBeenCalledTimes(2);
  });

  it('rejects a sequence that moves behind the resume cursor', async () => {
    const fetch = vi.fn(() =>
      Promise.resolve(sse([{ seq: 6, event: { type: 'session_created' } }])),
    );
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    await expect(first(streamEvents(config, SESSION, { startingAfter: 7 }))).rejects.toThrow(
      /behind resume cursor/i,
    );
  });

  it.each([
    [
      new Response('id: 1\ndata: {\n\n', {
        status: 200,
        headers: { 'Content-Type': 'text/event-stream' },
      }),
      /valid JSON/i,
    ],
    [
      new Response(new Uint8Array([0xc3, 0x28]), {
        status: 200,
        headers: { 'Content-Type': 'text/event-stream' },
      }),
      /UTF-8|SSE/i,
    ],
    [sse([{ seq: 1, event: { type: 'progress', stage: 'future' } }]), /progress stage/i],
    [
      new Response('unknown-field\n\n', {
        status: 200,
        headers: { 'Content-Type': 'text/event-stream' },
      }),
      /valid SSE/i,
    ],
  ])('reports malformed stream input %#', async (response, expected) => {
    const fetch = vi.fn(() => Promise.resolve(response));
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });
    await expect(first(streamEvents(config, SESSION))).rejects.toThrow(expected);
  });

  it.each([
    [
      new Response('', { status: 200, headers: { 'Content-Type': 'application/json' } }),
      /content type/i,
    ],
    [
      new Response(null, { status: 204, headers: { 'Content-Type': 'text/event-stream' } }),
      /HTTP 200/i,
    ],
    [
      new Response(null, { status: 200, headers: { 'Content-Type': 'text/event-stream' } }),
      /no body/i,
    ],
  ])('rejects an invalid stream handshake %#', async (response, expected) => {
    const fetch = vi.fn(() => Promise.resolve(response));
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });
    await expect(first(streamEvents(config, SESSION))).rejects.toThrow(expected);
  });

  it('honours a pre-aborted signal', async () => {
    const controller = new AbortController();
    controller.abort('done');
    const fetch = vi.fn();
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    await expect(
      first(streamEvents(config, SESSION, { signal: controller.signal })),
    ).rejects.toBeInstanceOf(RequestAborted);
    expect(fetch).not.toHaveBeenCalled();
  });

  it('times out a stalled stream handshake', async () => {
    const fetch = vi.fn(
      (_url: string | URL | Request, init?: RequestInit) =>
        new Promise<Response>((_resolve, reject) => {
          init?.signal?.addEventListener(
            'abort',
            () => reject(new DOMException('aborted', 'AbortError')),
            { once: true },
          );
        }),
    );
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    await expect(
      first(streamEvents(config, SESSION, { timeoutMs: 5, maxEmptyConnections: 1 })),
    ).rejects.toBeInstanceOf(TimeoutError);
  });

  it('does not continue a timed-out request-id hook into a late handshake', async () => {
    const requestId = deferred<string>();
    const requestIdProvider = vi.fn(() => requestId.promise);
    const credential = vi.fn(() => 'tok_late');
    const fetch = vi.fn();
    const config = resolveConfig({
      credential,
      fetch: fetch as unknown as typeof globalThis.fetch,
      requestIdProvider,
    });

    await expect(
      first(streamEvents(config, SESSION, { timeoutMs: 5, maxEmptyConnections: 1 })),
    ).rejects.toBeInstanceOf(TimeoutError);
    expect(requestIdProvider).toHaveBeenCalledOnce();

    requestId.resolve('caller-event-after-timeout');
    await nextTurn();

    expect(credential).not.toHaveBeenCalled();
    expect(fetch).not.toHaveBeenCalled();
  });

  it('does not read a stream body after its response observer times out', async () => {
    const observer = deferred<void>();
    const observerStarted = deferred<void>();
    const responseObserver = vi.fn(() => {
      observerStarted.resolve(undefined);
      return observer.promise;
    });
    const getReader = vi.fn();
    const response = {
      body: { getReader },
      headers: new Headers({ 'content-type': 'text/event-stream' }),
      ok: true,
      status: 200,
    } as unknown as Response;
    const fetch = vi.fn(() => Promise.resolve(response));
    const config = resolveConfig({
      fetch: fetch as unknown as typeof globalThis.fetch,
      responseObserver,
    });
    const pending = first(streamEvents(config, SESSION, { timeoutMs: 50, maxEmptyConnections: 1 }));
    await observerStarted.promise;

    await expect(pending).rejects.toBeInstanceOf(TimeoutError);
    observer.resolve(undefined);
    await nextTurn();

    expect(responseObserver).toHaveBeenCalledOnce();
    expect(getReader).not.toHaveBeenCalled();
  });

  it('aborts immediately while waiting to reconnect', async () => {
    const fetch = vi.fn(() => Promise.reject(new Error('offline')));
    const sleep = vi.fn(() => new Promise<void>(() => undefined));
    const controller = new AbortController();
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });
    const pending = first(
      streamEvents(config, SESSION, { signal: controller.signal, sleep, jitterRatio: 0 }),
    );
    await vi.waitFor(() => expect(sleep).toHaveBeenCalledOnce());

    controller.abort('cancelled');

    await expect(pending).rejects.toBeInstanceOf(RequestAborted);
  });

  it('validates reconnect policy before opening a connection', () => {
    const config = resolveConfig({ fetch: vi.fn() as unknown as typeof globalThis.fetch });
    expect(() => streamEvents(config, SESSION, { startingAfter: -1 })).toThrow(ValidationError);
    expect(() => streamEvents(config, SESSION, { initialBackoffMs: 20, maxBackoffMs: 10 })).toThrow(
      ValidationError,
    );
    expect(() => streamEvents(config, SESSION, { jitterRatio: 2 })).toThrow(ValidationError);
    expect(() => streamEvents(config, SESSION, { maxEmptyConnections: 0 })).toThrow(
      ValidationError,
    );
  });

  it('rejects an invalid random source rather than computing an unsafe delay', async () => {
    const fetch = vi.fn(() => Promise.reject(new Error('offline')));
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    await expect(
      first(streamEvents(config, SESSION, { random: () => Number.NaN })),
    ).rejects.toBeInstanceOf(ValidationError);
  });

  it('keeps a non-retryable API response typed', async () => {
    const fetch = vi.fn(() =>
      Promise.resolve(
        new Response(
          JSON.stringify({ status: 404, detail: 'session not found', code: 'not_found' }),
          {
            status: 404,
          },
        ),
      ),
    );
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    await expect(first(streamEvents(config, SESSION))).rejects.toBeInstanceOf(ApiError);
    expect(fetch).toHaveBeenCalledOnce();
  });
});
