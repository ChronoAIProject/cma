// @vitest-environment node
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  ProtocolError,
  RequestAborted,
  TimeoutError,
  TransportError,
  createClient,
} from '../index.js';
import {
  completed,
  events,
  harness,
  request,
  response,
  responseId,
  sse,
  wire,
} from './responses-v2-fixtures.js';

const prefix = () => sse(wire(events().slice(0, 5)));
const suffix = () => sse(wire(events().slice(5)));
const json = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), { status, headers: { 'content-type': 'application/json' } });
const methods = (fetch: ReturnType<typeof harness>['fetch']) =>
  fetch.mock.calls.map(([, init]) => init?.method);

afterEach(() => vi.useRealTimers());

describe('Responses v2 admitted stream recovery', () => {
  it('replays a dropped response body with current authorization and a fresh request id', async () => {
    const fetch = vi.fn<typeof globalThis.fetch>();
    const credentials = vi.fn().mockResolvedValueOnce('first').mockResolvedValueOnce('second');
    const requestIds = vi
      .fn()
      .mockReturnValueOnce('post-request')
      .mockReturnValueOnce('replay-request');
    const client = createClient({
      baseUrl: 'https://cma.example',
      fetch,
      credential: credentials,
      requestIdProvider: requestIds,
    });
    let pull = 0;
    fetch
      .mockResolvedValueOnce(
        sse(
          new ReadableStream<Uint8Array>({
            pull(controller) {
              if (pull++ === 0)
                controller.enqueue(new TextEncoder().encode(wire(events().slice(0, 5))));
              else controller.error(new TypeError('body connection closed'));
            },
          }),
        ),
      )
      .mockResolvedValueOnce(suffix());
    const onEvent = vi.fn();
    const onRecovery = vi.fn();
    await expect(client.responsesV2.create(request, { onEvent, onRecovery })).resolves.toEqual(
      completed,
    );
    expect(methods(fetch)).toEqual(['POST', 'GET']);
    expect(onEvent.mock.calls.map(([event]) => event)).toEqual(events());
    expect(
      fetch.mock.calls.map(([, init]) => new Headers(init?.headers).get('authorization')),
    ).toEqual(['Bearer first', 'Bearer second']);
    expect(onRecovery).toHaveBeenCalledWith(
      expect.objectContaining({
        initiatingRequestId: 'post-request',
        replayRequestId: 'replay-request',
        reason: 'transport',
        startingAfter: 4,
      }),
    );
  });

  it('recovers from an admitted receipt before the first durable frame', async () => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(sse(':\n\n')).mockResolvedValueOnce(sse());
    await expect(t.client.responsesV2.create(request, { onEvent: t.onEvent })).resolves.toEqual(
      completed,
    );
    expect(String(t.fetch.mock.calls[1]![0])).toBe(
      `https://cma.example/api/v2/responses/${responseId}/events`,
    );
    expect(t.onEvent.mock.calls.map(([event]) => event)).toEqual(events());
  });

  it('checks the terminal response after an empty replay', async () => {
    const t = harness();
    t.fetch
      .mockResolvedValueOnce(prefix())
      .mockResolvedValueOnce(sse(''))
      .mockResolvedValueOnce(json(completed));
    await expect(t.client.responsesV2.create(request, { onEvent: t.onEvent })).resolves.toEqual(
      completed,
    );
    expect(methods(t.fetch)).toEqual(['POST', 'GET', 'GET']);
    expect(String(t.fetch.mock.calls[2]![0])).toBe(
      `https://cma.example/api/v2/responses/${responseId}`,
    );
    expect(t.onEvent.mock.calls.map(([event]) => event)).toEqual(events().slice(0, 5));
  });

  it('does not treat an empty replay of a still-working response as terminal', async () => {
    vi.useFakeTimers();
    const t = harness();
    t.fetch
      .mockResolvedValueOnce(prefix())
      .mockResolvedValueOnce(sse(''))
      .mockResolvedValueOnce(json({ ...response, status: 'in_progress' }))
      .mockResolvedValueOnce(suffix());
    const outcome = t.client.responsesV2.create(request, { onEvent: t.onEvent });
    const assertion = expect(outcome).resolves.toEqual(completed);
    await vi.runAllTimersAsync();
    await assertion;
    expect(methods(t.fetch)).toEqual(['POST', 'GET', 'GET', 'GET']);
    expect(String(t.fetch.mock.calls[3]![0])).toContain('starting_after=4');
    expect(t.onEvent.mock.calls.map(([event]) => event)).toEqual(events());
  });

  it.each([401, 403, 404])('propagates HTTP %s after authorization changes', async (status) => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(prefix()).mockResolvedValueOnce(
      json(
        {
          error: {
            type: 'invalid_request_error',
            code: 'resource_not_found',
            message: 'refused',
            param: null,
          },
        },
        status,
      ),
    );
    await expect(
      t.client.responsesV2.create(request, { onEvent: t.onEvent }),
    ).rejects.toMatchObject({ status });
    expect(methods(t.fetch)).toEqual(['POST', 'GET']);
  });

  it.each<Record<string, string>>([
    { 'x-cma-command-id': 'cmd_other' },
    { 'x-cma-response-id': 'resp_other' },
  ])('refuses a different receipt on replay', async (headers) => {
    const t = harness();
    t.fetch
      .mockResolvedValueOnce(prefix())
      .mockResolvedValueOnce(sse(wire(events().slice(5)), headers));
    await expect(
      t.client.responsesV2.create(request, { onEvent: t.onEvent }),
    ).rejects.toBeInstanceOf(ProtocolError);
    expect(methods(t.fetch)).toEqual(['POST', 'GET']);
  });

  it('retains validation across a replay in the middle of an output item', async () => {
    const t = harness();
    const remainder = events().slice(5);
    remainder[0] = { ...remainder[0], text: 'different text' };
    t.fetch.mockResolvedValueOnce(prefix()).mockResolvedValueOnce(sse(wire(remainder)));
    await expect(
      t.client.responsesV2.create(request, { onEvent: t.onEvent }),
    ).rejects.toBeInstanceOf(ProtocolError);
    expect(methods(t.fetch)).toEqual(['POST', 'GET']);
  });

  it('does not resubmit or recover before a valid receipt', async () => {
    const t = harness();
    t.fetch.mockRejectedValueOnce(new TypeError('connection lost'));
    await expect(
      t.client.responsesV2.create(request, { onEvent: t.onEvent }),
    ).rejects.toBeInstanceOf(TransportError);
    expect(methods(t.fetch)).toEqual(['POST']);
  });

  it('does not recover a nonretryable observation condition', async () => {
    const t = harness();
    const event = {
      type: 'error',
      sequence_number: 1,
      code: 'response_pagination_required',
      message: 'Use output pages',
      param: null,
    };
    t.fetch.mockResolvedValueOnce(sse(wire([events()[0]!, event])));
    await expect(
      t.client.responsesV2.create(request, { onEvent: t.onEvent }),
    ).rejects.toBeInstanceOf(TransportError);
    expect(t.onEvent.mock.calls.map(([value]) => value)).toEqual([events()[0], event]);
    expect(methods(t.fetch)).toEqual(['POST']);
  });

  it('honors cancellation before issuing a recovery request', async () => {
    const t = harness();
    const caller = new AbortController();
    t.fetch.mockResolvedValueOnce(prefix());
    await expect(
      t.client.responsesV2.create(request, {
        signal: caller.signal,
        onEvent: t.onEvent,
        onRecovery: () => caller.abort(),
      }),
    ).rejects.toBeInstanceOf(RequestAborted);
    expect(methods(t.fetch)).toEqual(['POST']);
  });

  it('caps successive interrupted replays and never repeats the submission', async () => {
    vi.useFakeTimers();
    const t = harness();
    t.fetch.mockResolvedValueOnce(prefix()).mockRejectedValue(new TypeError('transport down'));
    const recovery = vi.fn();
    const outcome = t.client.responsesV2.create(request, {
      onEvent: t.onEvent,
      onRecovery: recovery,
    });
    const assertion = expect(outcome).rejects.toBeInstanceOf(TransportError);
    await vi.runAllTimersAsync();
    await assertion;
    expect(methods(t.fetch)).toEqual(['POST', 'GET', 'GET', 'GET', 'GET']);
    expect(recovery).toHaveBeenCalledTimes(4);
    expect(recovery.mock.calls.every(([notice]) => notice.startingAfter === 4)).toBe(true);
  });

  it('bounds the recovery body by the total deadline', async () => {
    vi.useFakeTimers();
    const t = harness();
    t.fetch
      .mockResolvedValueOnce(prefix())
      .mockResolvedValueOnce(sse(new ReadableStream({ pull: () => new Promise(() => {}) })));
    const outcome = t.client.responsesV2.create(request, { onEvent: t.onEvent });
    const assertion = expect(outcome).rejects.toBeInstanceOf(TimeoutError);
    await vi.advanceTimersByTimeAsync(120_000);
    await assertion;
    expect(methods(t.fetch)).toEqual(['POST', 'GET']);
  });
});
