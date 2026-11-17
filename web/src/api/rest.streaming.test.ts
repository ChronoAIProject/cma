/** @vitest-environment node */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError, RequestAborted, TransportError, createClient } from '@cma/client';
import { createRestWebClient } from './rest';
import type { AgentLiveEvent } from './types';
import { conversationFromTranscript, foldLiveEvent } from '../agent/conversation';

const threadId = 'agent-stream';
const request = {
  threadId,
  runId: 'web-correlation',
  messages: [{ role: 'user' as const, content: 'Review the changes' }],
};

function frame(type: string, cursor?: number, fields: Record<string, unknown> = {}): string {
  return `${cursor === undefined ? '' : `id: ${String(cursor)}\n`}data: ${JSON.stringify({
    type,
    threadId,
    runId: '1',
    ...fields,
  })}\n\n`;
}

function sse(body: string): Response {
  return new Response(body, { headers: { 'content-type': 'text/event-stream' } });
}

function harness() {
  const fetch = vi.fn<typeof globalThis.fetch>();
  const client = createRestWebClient(
    createClient({ fetch, requestIdProvider: () => 'web-stream' }),
  );
  const events: AgentLiveEvent[] = [];
  return { fetch, client, events, onEvent: (event: AgentLiveEvent) => events.push(event) };
}

afterEach(() => vi.useRealTimers());

describe('production Agent streaming binding', () => {
  it('submits through public AG-UI and preserves live and durable projection fields', async () => {
    const h = harness();
    h.fetch.mockResolvedValueOnce(
      sse(
        frame('RUN_STARTED', 1) +
          frame('TEXT_MESSAGE_CONTENT', undefined, {
            'cma:live': true,
            'cma:after': 1,
            messageId: 'live-1',
            delta: 'Reviewed',
          }) +
          frame('TEXT_MESSAGE_START', undefined, { messageId: 'msg-2', role: 'assistant' }) +
          frame('TEXT_MESSAGE_CONTENT', undefined, { messageId: 'msg-2', delta: 'Reviewed' }) +
          frame('TEXT_MESSAGE_END', 2, { messageId: 'msg-2' }) +
          frame('RUN_FINISHED', 3),
      ),
    );
    await h.client.runAgent(request, { onEvent: h.onEvent });
    expect(h.fetch).toHaveBeenCalledOnce();
    const [url, init] = h.fetch.mock.calls[0] ?? [];
    expect(url).toBe('/api/v1/agui/run');
    expect(init).toMatchObject({ method: 'POST', credentials: 'include', redirect: 'error' });
    expect(JSON.parse(String(init?.body))).toEqual(request);
    expect(new Headers(init?.headers).get('x-request-id')).toBe('web-stream');
    expect(new Headers(init?.headers).has('idempotency-key')).toBe(false);
    expect(h.events.map((event) => event.cursor)).toEqual([
      1,
      undefined,
      undefined,
      undefined,
      2,
      3,
    ]);
    expect(h.events[1]).toMatchObject({ 'cma:live': true, 'cma:after': 1, delta: 'Reviewed' });
  });

  it('passes approval decisions to the same public run operation', async () => {
    const h = harness();
    h.fetch.mockResolvedValueOnce(sse(frame('RUN_STARTED') + frame('RUN_FINISHED', 9)));
    const decision = {
      threadId,
      runId: 'approval-correlation',
      resume: [
        {
          interruptId: 'approval-1',
          status: 'resolved' as const,
          payload: { decision: 'deny' as const },
        },
      ],
    };
    await h.client.runAgent(decision, { onEvent: h.onEvent });
    expect(JSON.parse(String(h.fetch.mock.calls[0]?.[1]?.body))).toEqual(decision);
    expect(h.events[0]?.cursor).toBeUndefined();
  });

  it('replays from the view cursor and cancels the underlying reader on navigation', async () => {
    const h = harness();
    const controller = new AbortController();
    const cancel = vi.fn();
    h.fetch.mockResolvedValueOnce(
      new Response(
        new ReadableStream({
          start(stream) {
            stream.enqueue(new TextEncoder().encode(frame('RUN_STARTED', 8)));
          },
          cancel,
        }),
        { headers: { 'content-type': 'text/event-stream' } },
      ),
    );
    await expect(
      h.client.replayAgent(threadId, {
        after: 7,
        signal: controller.signal,
        onEvent(event) {
          h.onEvent(event);
          controller.abort();
        },
      }),
    ).rejects.toBeInstanceOf(RequestAborted);
    expect(h.fetch.mock.calls[0]?.[0]).toBe(
      '/api/v1/agui/events?thread_id=agent-stream&starting_after=7&follow=true',
    );
    expect(h.fetch.mock.calls[0]?.[1]?.method).toBe('GET');
    expect(new Headers(h.fetch.mock.calls[0]?.[1]?.headers).has('last-event-id')).toBe(false);
    expect(cancel).toHaveBeenCalledOnce();
    expect(h.events[0]?.cursor).toBe(8);
  });

  it('reconnects only read-only replay from acknowledged durable progress', async () => {
    vi.useFakeTimers();
    const h = harness();
    const controller = new AbortController();
    h.fetch.mockResolvedValueOnce(sse(frame('RUN_STARTED', 4) + frame('TEXT_MESSAGE_START')));
    h.fetch.mockResolvedValueOnce(sse(frame('RUN_FINISHED', 6)));
    const result = h.client
      .replayAgent(threadId, {
        after: 3,
        signal: controller.signal,
        onEvent(event) {
          h.onEvent(event);
          if (event.cursor === 6) controller.abort();
        },
      })
      .catch((error: unknown) => error);
    await vi.runAllTimersAsync();
    expect(await result).toBeInstanceOf(RequestAborted);
    expect(h.fetch.mock.calls.map(([url]) => url)).toEqual([
      '/api/v1/agui/events?thread_id=agent-stream&starting_after=3&follow=true',
      '/api/v1/agui/events?thread_id=agent-stream&starting_after=4&follow=true',
    ]);
    expect(h.fetch.mock.calls.every(([, init]) => init?.method === 'GET')).toBe(true);
    expect(h.events.map((event) => event.type)).toEqual(['RUN_STARTED', 'RUN_FINISHED']);
  });

  it('surfaces uncertain run outcomes without silently resubmitting input', async () => {
    const h = harness();
    h.fetch.mockResolvedValueOnce(sse(frame('RUN_STARTED', 1)));
    await expect(h.client.runAgent(request, { onEvent: h.onEvent })).rejects.toBeInstanceOf(
      TransportError,
    );
    expect(h.fetch).toHaveBeenCalledOnce();
  });

  it('preserves authentication problems for sign-in recovery', async () => {
    const h = harness();
    h.fetch.mockResolvedValueOnce(
      new Response(JSON.stringify({ status: 401, code: 'unauthorized', detail: 'Sign in' }), {
        status: 401,
        headers: { 'content-type': 'application/problem+json', 'x-request-id': 'server-stream' },
      }),
    );
    const error = await h.client
      .replayAgent(threadId, { onEvent: h.onEvent })
      .catch((cause: unknown) => cause);
    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({ status: 401, code: 'unauthorized', requestId: 'server-stream' });
    expect(h.fetch).toHaveBeenCalledOnce();
  });
});

it.each([false, true])(
  'reassembles chunks after reconnect without changing healthy outcomes (read error: %s)',
  async (readError) => {
    vi.useFakeTimers();
    const h = harness();
    const chunk = 'retained λ output'.repeat(900);
    const start = frame('TEXT_MESSAGE_START', undefined, { messageId: 'msg_5', role: 'assistant' });
    const content = frame('TEXT_MESSAGE_CONTENT', undefined, { messageId: 'msg_5', delta: chunk });
    let sent = false;
    h.fetch.mockResolvedValueOnce(
      readError
        ? new Response(
            new ReadableStream<Uint8Array>({
              pull(controller) {
                if (sent) controller.error(new Error('history transport interrupted'));
                else {
                  sent = true;
                  controller.enqueue(new TextEncoder().encode(start + content));
                }
              },
            }),
            { headers: { 'content-type': 'text/event-stream' } },
          )
        : sse(start + content),
    );
    h.fetch.mockResolvedValueOnce(
      sse(
        start +
          content +
          content +
          frame('TEXT_MESSAGE_END', 5, { messageId: 'msg_5' }) +
          frame('RUN_FINISHED', 6, { outcome: { type: 'complete' } }),
      ),
    );
    h.fetch.mockResolvedValueOnce(
      new Response(JSON.stringify({ status: 401, detail: 'Sign in', code: 'unauthorized' }), {
        status: 401,
      }),
    );
    let state = conversationFromTranscript({
      agentId: threadId,
      throughSeq: 4,
      turns: [
        {
          turnSequence: 1,
          startedAt: '2026-09-06T00:00:00Z',
          ownerMessage: 'input',
          agentMessages: [],
          messages: [],
          outcome: { type: 'completed' },
        },
      ],
    });
    const before = state.turnOutcomes;
    const result = h.client
      .replayAgent(threadId, {
        after: 4,
        onEvent(event) {
          expect(event.type).not.toBe('RUN_ERROR');
          state = foldLiveEvent(state, event, threadId);
        },
      })
      .catch((error: unknown) => error);
    await vi.runAllTimersAsync();
    expect(await result).toBeInstanceOf(ApiError);
    expect(h.fetch.mock.calls.map(([url]) => String(url))).toEqual([
      expect.stringContaining('starting_after=4'),
      expect.stringContaining('starting_after=4'),
      expect.stringContaining('starting_after=6'),
    ]);
    expect(state.messages.filter(({ id }) => id === 'msg_5')).toMatchObject([
      { content: chunk + chunk, status: 'complete' },
    ]);
    expect(state.turnOutcomes).toEqual(before);
    expect(state.recovery.some((item) => item.title === 'Agent run failed')).toBe(false);
  },
);
