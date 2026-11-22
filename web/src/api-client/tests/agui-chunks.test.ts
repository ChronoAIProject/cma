// @vitest-environment node
import { afterEach, expect, it, vi } from 'vitest';
import { AgentConversations, type AgentConversationEvent } from '../agui/index.js';
import { resolveConfig } from '../config.js';
import { ApiError, ProtocolError } from '../errors.js';

const agent = 'agt_stream';
function frame(type: string, cursor?: number, fields: Record<string, unknown> = {}): string {
  return `${cursor === undefined ? '' : `id: ${String(cursor)}\n`}data: ${JSON.stringify({ type, threadId: agent, runId: '1', ...fields })}\n\n`;
}
function sse(body: string): Response {
  return new Response(body, { headers: { 'content-type': 'text/event-stream' } });
}
function harness() {
  const fetch = vi.fn<typeof globalThis.fetch>();
  const client = new AgentConversations(resolveConfig({ baseUrl: 'https://cma.example', fetch }));
  const events: AgentConversationEvent[] = [];
  return {
    fetch,
    client,
    events,
    onEvent(event: AgentConversationEvent) {
      events.push(event);
    },
  };
}
afterEach(() => vi.useRealTimers());

it.each([false, true])(
  'reassembles chunks without a run failure after interruption (read error: %s)',
  async (readError) => {
    vi.useFakeTimers();
    const h = harness();
    const chunk = 'retained λ output'.repeat(900);
    const start = frame('TEXT_MESSAGE_START', undefined, { messageId: 'msg_2', role: 'assistant' });
    const content = frame('TEXT_MESSAGE_CONTENT', undefined, { messageId: 'msg_2', delta: chunk });
    let sent = false;
    const prefix = frame('RUN_STARTED', 1) + start + content;
    h.fetch.mockResolvedValueOnce(
      readError
        ? new Response(
            new ReadableStream<Uint8Array>({
              pull(controller) {
                if (sent) controller.error(new Error('history transport interrupted'));
                else {
                  sent = true;
                  controller.enqueue(new TextEncoder().encode(prefix));
                }
              },
            }),
            { headers: { 'content-type': 'text/event-stream' } },
          )
        : sse(prefix),
    );
    h.fetch.mockResolvedValueOnce(
      sse(start + content + content + frame('TEXT_MESSAGE_END', 2, { messageId: 'msg_2' })),
    );
    h.fetch.mockResolvedValueOnce(
      new Response(JSON.stringify({ status: 401, detail: 'Sign in', code: 'unauthorized' }), {
        status: 401,
      }),
    );
    const result = h.client.replay(agent, { onEvent: h.onEvent }).catch((error: unknown) => error);
    await vi.runAllTimersAsync();
    expect(await result).toBeInstanceOf(ApiError);
    expect(h.fetch.mock.calls.map(([url]) => String(url))).toEqual([
      expect.stringContaining('starting_after=0'),
      expect.stringContaining('starting_after=1'),
      expect.stringContaining('starting_after=2'),
    ]);
    expect(h.events.map(({ type }) => type)).toEqual([
      'RUN_STARTED',
      'TEXT_MESSAGE_START',
      'TEXT_MESSAGE_CONTENT',
      'TEXT_MESSAGE_CONTENT',
      'TEXT_MESSAGE_END',
    ]);
    expect(
      h.events
        .filter(({ type }) => type === 'TEXT_MESSAGE_CONTENT')
        .map(({ delta }) => delta)
        .join(''),
    ).toBe(chunk + chunk);
    expect(h.events.map(({ cursor }) => cursor)).toEqual([1, undefined, undefined, undefined, 2]);
  },
);

it('accepts a retained one-MiB message split into 8-KiB committed content frames', async () => {
  const h = harness();
  const chunk = 'x'.repeat(8192);
  const content = frame('TEXT_MESSAGE_CONTENT', undefined, { messageId: 'msg_2', delta: chunk });
  h.fetch.mockResolvedValueOnce(
    sse(
      frame('TEXT_MESSAGE_START', undefined, { messageId: 'msg_2', role: 'assistant' }) +
        content.repeat(128) +
        frame('TEXT_MESSAGE_END', 2, { messageId: 'msg_2' }),
    ),
  );
  await h.client.replay(agent, { follow: false, onEvent: h.onEvent });
  expect(
    h.events
      .filter(({ type }) => type === 'TEXT_MESSAGE_CONTENT')
      .map(({ delta }) => delta)
      .join(''),
  ).toBe(chunk.repeat(128));
  expect(h.events.slice(0, -1).every(({ cursor }) => cursor === undefined)).toBe(true);
  expect(h.events.at(-1)?.cursor).toBe(2);
});

it('refuses excessive text fragments and does not group different messages to bypass projection bounds', async () => {
  for (const sameMessage of [false, true]) {
    const h = harness();
    const count = sameMessage ? 513 : 33;
    const body = Array.from({ length: count }, (_, index) =>
      frame('TEXT_MESSAGE_CONTENT', undefined, {
        messageId: sameMessage ? 'msg_2' : `msg_${String(index)}`,
        delta: 'x',
      }),
    ).join('');
    h.fetch.mockResolvedValueOnce(sse(body + frame('TEXT_MESSAGE_END', 2)));
    await expect(
      h.client.replay(agent, { follow: false, onEvent: h.onEvent }),
    ).rejects.toBeInstanceOf(ProtocolError);
    expect(h.events).toEqual([]);
  }
});

it('retains the four-MiB projection byte limit across individually valid text chunks', async () => {
  const h = harness();
  let emitted = 0;
  h.fetch.mockResolvedValueOnce(
    new Response(
      new ReadableStream<Uint8Array>({
        pull(controller) {
          if (emitted++ < 129) {
            controller.enqueue(
              new TextEncoder().encode(
                frame('TEXT_MESSAGE_CONTENT', undefined, {
                  messageId: 'msg_2',
                  delta: 'x'.repeat(32768),
                }),
              ),
            );
          } else {
            controller.enqueue(
              new TextEncoder().encode(frame('TEXT_MESSAGE_END', 2, { messageId: 'msg_2' })),
            );
            controller.close();
          }
        },
      }),
      { headers: { 'content-type': 'text/event-stream' } },
    ),
  );
  await expect(
    h.client.replay(agent, { follow: false, onEvent: h.onEvent }),
  ).rejects.toBeInstanceOf(ProtocolError);
  expect(h.events).toEqual([]);
});
