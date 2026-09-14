import { afterEach, describe, expect, it, vi } from 'vitest';
import { ProtocolError, RequestAborted, TimeoutError, TransportError } from '../src/index.js';
import {
  agentId,
  response,
  completed,
  frame,
  opening,
  ending,
  request,
  sse,
  harness,
} from './responses-fixtures.js';

afterEach(() => vi.useRealTimers());

describe('Responses stream transport integrity', () => {
  it('returns and cancels the reader after acknowledging a terminal frame without waiting for EOF', async () => {
    const t = harness();
    const cancel = vi.fn();
    const body = new ReadableStream({
      start(controller) {
        controller.enqueue(new TextEncoder().encode(opening + ending));
      },
      cancel,
    });
    t.fetch.mockResolvedValueOnce(sse(body));
    expect(await t.client.responses.create(request, { onEvent: t.onEvent })).toEqual(completed);
    expect(cancel).toHaveBeenCalledOnce();
    expect(body.locked).toBe(false);
  });
  it('handles split UTF-8 and SSE frames and releases the body reader', async () => {
    const t = harness();
    const bytes = new TextEncoder().encode(opening + ending);
    const body = new ReadableStream({
      start(controller) {
        for (const byte of bytes) controller.enqueue(new Uint8Array([byte]));
        controller.close();
      },
    });
    t.fetch.mockResolvedValueOnce(sse(body));
    expect(await t.client.responses.create(request, { onEvent: t.onEvent })).toEqual(completed);
    expect(body.locked).toBe(false);
  });

  it.each([opening, '', opening + 'event: response.completed\ndata: {"partial":'])(
    'never resubmits input when a stream closes without a complete terminal event',
    async (body) => {
      const t = harness();
      t.fetch.mockResolvedValueOnce(sse(body));
      await expect(
        t.client.responses.create(request, { onEvent: t.onEvent }),
      ).rejects.toBeInstanceOf(TransportError);
      expect(t.fetch).toHaveBeenCalledTimes(1);
    },
  );

  it.each([
    ending,
    opening + opening,
    opening + frame('response.completed', 3, { response: completed }),
    opening + ending + frame('response.completed', 2, { response: completed }),
    'id: 1\n' + opening + ending,
    opening.replace('event: response.created', 'event: response.failed') + ending,
    frame('response.created', 0, { response: { ...response, id: 'resp_agt_other_1' } }),
    opening +
      frame('response.completed', 1, { response: { ...completed, id: `resp_${agentId}_2` } }),
    opening + frame('response.completed', 1, { response }),
    frame('response.output_text.delta', 0, {
      item_id: 'msg_example',
      output_index: 0,
      content_index: 0,
      delta: 'hello',
    }),
    opening +
      frame('response.cma_chat.tool_activity', 1, {
        response_id: 'wrong',
        item: {
          id: 'tool_example',
          type: 'cma_chat.tool_activity',
          call_id: 'c1',
          kind: 'read',
          target: null,
          status: 'completed',
          duration_ms: null,
        },
      }),
    opening + frame('response.invented', 1, {}),
    'data: not-json\n\n',
  ])('refuses invalid event identity, envelope, order, or shape', async (body) => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(sse(body));
    await expect(t.client.responses.create(request, { onEvent: t.onEvent })).rejects.toBeInstanceOf(
      ProtocolError,
    );
    expect(t.fetch).toHaveBeenCalledTimes(1);
  });

  it('rejects invalid UTF-8 and oversized framing without an unbounded queue', async () => {
    for (const body of [
      new Uint8Array([0xff]),
      'data: ' + 'x'.repeat(2 * 1024 * 1024 + 1),
      'x'.repeat(4 * 1024 * 1024 + 1),
    ]) {
      const t = harness();
      t.fetch.mockResolvedValueOnce(sse(body));
      await expect(
        t.client.responses.create(request, { onEvent: t.onEvent }),
      ).rejects.toBeInstanceOf(ProtocolError);
    }
  });

  it('does not swallow consumer failure or reinterpret it as an Agent outcome', async () => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(sse());
    t.onEvent.mockRejectedValueOnce(new Error('consumer refused'));
    await expect(t.client.responses.create(request, { onEvent: t.onEvent })).rejects.toThrow(
      'did not acknowledge',
    );
    expect(t.fetch).toHaveBeenCalledTimes(1);
  });

  it('preserves transport, media, HTTP problem, and unexpected-status failures without retry', async () => {
    const t = harness();
    t.fetch.mockRejectedValueOnce(new Error('disconnected'));
    await expect(t.client.responses.create(request, { onEvent: t.onEvent })).rejects.toBeInstanceOf(
      TransportError,
    );
    t.fetch.mockResolvedValueOnce(Response.json({}));
    await expect(t.client.responses.create(request, { onEvent: t.onEvent })).rejects.toBeInstanceOf(
      ProtocolError,
    );
    t.fetch.mockResolvedValueOnce(new Response('', { status: 201 }));
    await expect(t.client.responses.create(request, { onEvent: t.onEvent })).rejects.toBeInstanceOf(
      ProtocolError,
    );
    t.fetch.mockResolvedValueOnce(
      Response.json(
        { status: 409, code: 'previous_response_id_conflict', detail: 'Prior response changed.' },
        { status: 409 },
      ),
    );
    await expect(t.client.responses.create(request, { onEvent: t.onEvent })).rejects.toMatchObject({
      code: 'previous_response_id_conflict',
    });
    expect(t.fetch).toHaveBeenCalledTimes(4);
  });

  it('bounds the handshake but allows the caller to abort an established stream', async () => {
    vi.useFakeTimers();
    const t = harness();
    t.fetch.mockImplementationOnce(() => new Promise(() => {}));
    const waiting = t.client.responses
      .create(request, { timeoutMs: 10, onEvent: t.onEvent })
      .catch((error) => error);
    await vi.advanceTimersByTimeAsync(11);
    expect(await waiting).toBeInstanceOf(TimeoutError);
    const controller = new AbortController();
    const body = new ReadableStream({
      start(stream) {
        stream.enqueue(new TextEncoder().encode(opening));
      },
    });
    t.fetch.mockResolvedValueOnce(sse(body));
    const active = t.client.responses
      .create(request, { timeoutMs: 10, signal: controller.signal, onEvent: t.onEvent })
      .catch((error) => error);
    await vi.advanceTimersByTimeAsync(11);
    expect(t.onEvent).toHaveBeenCalledTimes(1);
    controller.abort();
    expect(await active).toBeInstanceOf(RequestAborted);
    expect(body.locked).toBe(false);
  });
});
