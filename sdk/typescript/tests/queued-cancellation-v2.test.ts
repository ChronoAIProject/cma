import { describe, expect, it, vi } from 'vitest';
import { ProtocolError, RequestAborted, TransportError, ValidationError } from '../src/index.js';
import {
  agent,
  response,
  route,
  intent,
  observation,
  receipt,
  harness,
  json,
  set,
} from './queued-cancellation-v2-fixtures.js';

describe('queued response cancellation', () => {
  it('observes queued and unbound Channel waiting work without submitting a mutation', async () => {
    const t = harness();
    for (const status of ['queued', 'waiting_action']) {
      const body = observation();
      set(body, 'response.status', status);
      t.fetch.mockResolvedValueOnce(json(body));
      expect(await t.api.observeQueuedCancellation(agent, response)).toEqual(body);
    }
    for (const [url, options] of t.fetch.mock.calls) {
      expect(String(url)).toBe(`https://cma.example/gateway${route}`);
      expect(options!.method).toBe('GET');
      expect(options!.body).toBeUndefined();
    }
  });
  it('submits the exact source intent and retained key and preserves current source status on replay', async () => {
    const t = harness();
    const original = receipt();
    for (const status of ['cancelled', 'completed']) {
      const body = structuredClone(original);
      set(body, 'response.status', status);
      t.fetch.mockResolvedValueOnce(json(body));
      expect(
        await t.api.cancelQueuedResponse(agent, response, intent, { idempotencyKey: 'saved key' }),
      ).toEqual(body);
      const [url, options] = t.fetch.mock.calls.at(-1)!;
      expect(String(url)).toBe(`https://cma.example/gateway${route}`);
      expect(options!.method).toBe('POST');
      expect(JSON.parse(String(options!.body))).toEqual(intent);
      expect(new Headers(options!.headers).get('idempotency-key')).toBe('saved key');
    }
  });
  it('does not retry a lost reply and explicitly recovers with the identical key and body', async () => {
    const t = harness();
    t.fetch.mockRejectedValueOnce(new Error('lost reply')).mockResolvedValueOnce(json(receipt()));
    const options = { idempotencyKey: 'persisted-key' };
    await expect(
      t.api.cancelQueuedResponse(agent, response, intent, options),
    ).rejects.toBeInstanceOf(TransportError);
    expect(t.fetch).toHaveBeenCalledOnce();
    await t.api.cancelQueuedResponse(agent, response, intent, options);
    expect(t.fetch.mock.calls[0]![1]!.body).toBe(t.fetch.mock.calls[1]![1]!.body);
    expect(new Headers(t.fetch.mock.calls[1]![1]!.headers).get('idempotency-key')).toBe(
      options.idempotencyKey,
    );
  });
  it('captures mutable JavaScript intent before credential or network IO', async () => {
    const t = harness();
    let release!: (reply: Response) => void;
    t.fetch.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          release = resolve;
        }),
    );
    const body = { ...intent };
    const options = { idempotencyKey: 'saved-key' };
    const pending = t.api.cancelQueuedResponse(agent, response, body, options);
    body.source_command_id = 'changed';
    body.expected_command_revision += 1;
    options.idempotencyKey = 'changed-key';
    await vi.waitFor(() => expect(t.fetch).toHaveBeenCalledOnce());
    release(json(receipt()));
    await pending;
    expect(JSON.parse(String(t.fetch.mock.calls[0]![1]!.body))).toEqual(intent);
    expect(new Headers(t.fetch.mock.calls[0]![1]!.headers).get('idempotency-key')).toBe(
      'saved-key',
    );
  });
  it.each([
    [409, 'queued_response_cancellation_conflict'],
    [409, 'queued_response_cancellation_not_admitted'],
    [409, 'agent_command_key_conflict'],
    [422, 'invalid_queued_cancellation_request'],
    [404, 'response_not_found'],
    [401, 'not_authenticated'],
    [503, 'response_unavailable'],
  ])('preserves %s %s without automatic mutation retries', async (status, code) => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(
      new Response(
        JSON.stringify({
          error: { type: 'invalid_request_error', code, message: 'Refused', param: null },
        }),
        { status, headers: { 'Retry-After': '1' } },
      ),
    );
    await expect(
      t.api.cancelQueuedResponse(agent, response, intent, { idempotencyKey: 'saved' }),
    ).rejects.toMatchObject({ code });
    expect(t.fetch).toHaveBeenCalledOnce();
  });
  it.each(['', ' ', ' key', 'key ', 'key\n', 'k'.repeat(257), '😀'])(
    'rejects invalid or normalized key %s before HTTP',
    async (key) => {
      const t = harness();
      await expect(
        t.api.cancelQueuedResponse(agent, response, intent, { idempotencyKey: key }),
      ).rejects.toBeInstanceOf(ValidationError);
      expect(t.fetch).not.toHaveBeenCalled();
    },
  );
  it.each([
    { ...intent, actor: 'forged' },
    { ...intent, expected_agent_revision: 1 },
    { ...intent, expected_command_revision: 0 },
    { ...intent, expected_command_revision: Number.MAX_SAFE_INTEGER + 1 },
    { ...intent, source_command_id: '' },
    { ...intent, source_command_id: 'bad\n' },
  ])('rejects malformed source intent before HTTP', async (body) => {
    const t = harness();
    await expect(
      t.api.cancelQueuedResponse(agent, response, body, { idempotencyKey: 'saved' }),
    ).rejects.toBeInstanceOf(ValidationError);
    expect(t.fetch).not.toHaveBeenCalled();
  });
  it('accepts the maximum retained header length and refuses other success statuses', async () => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(json(receipt()));
    await t.api.cancelQueuedResponse(agent, response, intent, { idempotencyKey: 'k'.repeat(256) });
    t.fetch.mockResolvedValueOnce(json(receipt(), 202));
    await expect(
      t.api.cancelQueuedResponse(agent, response, intent, { idempotencyKey: 'saved' }),
    ).rejects.toBeInstanceOf(ProtocolError);
  });
  it('aborting a local observation never submits cancellation', async () => {
    const t = harness();
    await expect(
      t.api.observeQueuedCancellation(agent, response, { signal: AbortSignal.abort() }),
    ).rejects.toBeInstanceOf(RequestAborted);
    expect(t.fetch).not.toHaveBeenCalled();
  });
});
