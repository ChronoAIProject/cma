import { expect, it, vi } from 'vitest';
import { createClient, ValidationError } from '../src/index.js';
import { agentId, request as responseRequest, sse as responseSse } from './responses-fixtures.js';

function aguiSse() {
  const frame = (type: string, cursor: number) =>
    `id: ${String(cursor)}\ndata: ${JSON.stringify({ type, threadId: agentId, runId: '4' })}\n\n`;
  return new Response(frame('RUN_STARTED', 4) + frame('RUN_FINISHED', 7), {
    headers: { 'content-type': 'text/event-stream' },
  });
}

it('refuses an invalid suggestion answer condition before AG-UI admission', async () => {
  const fetch = vi.fn<typeof globalThis.fetch>().mockImplementation(async () => aguiSse());
  const client = createClient({ fetch });
  await expect(
    client.agui.run(
      {
        threadId: agentId,
        messages: [{ role: 'user', content: 'Add criteria.' }],
        forwardedProps: { answerContext: { turn_sequence: 4, answer_sequence: 2 } },
      },
      { idempotencyKey: 'suggestion-key', onEvent: () => {} },
    ),
  ).rejects.toThrow();
  expect(fetch).not.toHaveBeenCalled();
});

it.each(['agui', 'responses'] as const)(
  '%s captures input, Agent identity and key before credential resolution',
  async (protocol) => {
    let release!: (token: string) => void;
    let entered!: () => void;
    const waiting = new Promise<void>((resolve) => {
      entered = resolve;
    });
    const credential = vi.fn(() => {
      entered();
      return new Promise<string>((resolve) => {
        release = resolve;
      });
    });
    const fetch = vi
      .fn<typeof globalThis.fetch>()
      .mockImplementation(async () => (protocol === 'agui' ? aguiSse() : responseSse()));
    const client = createClient({ baseUrl: 'https://cma.example', credential, fetch });
    const run = {
      threadId: agentId,
      runId: 'correlation',
      messages: [{ role: 'user' as const, content: 'Original' }],
    };
    const response = {
      ...responseRequest,
      conversation: String(responseRequest.conversation),
      input: 'Original',
      metadata: { retained: 'original' },
    };
    const options = { idempotencyKey: 'retained-key', onEvent: vi.fn() };
    const promise =
      protocol === 'agui'
        ? client.agui.run(run, options)
        : client.responses.create(response, options);
    await waiting;
    run.threadId = 'agt_changed';
    run.messages[0]!.content = 'Changed';
    response.conversation = 'agt_changed';
    response.input = 'Changed';
    response.metadata.retained = 'changed';
    options.idempotencyKey = 'changed-key';
    release('selected-credential');
    await promise;
    expect(fetch).toHaveBeenCalledOnce();
    const init = fetch.mock.calls[0]![1]!;
    expect(new Headers(init.headers).get('Idempotency-Key')).toBe('retained-key');
    const sent = JSON.parse(String(init.body)) as Record<string, unknown>;
    if (protocol === 'agui') {
      expect(sent).toEqual({
        threadId: agentId,
        runId: 'correlation',
        messages: [{ role: 'user', content: 'Original' }],
      });
    } else {
      expect(sent).toEqual({
        ...responseRequest,
        input: 'Original',
        metadata: { retained: 'original' },
      });
    }
  },
);

it.each(['', 'x'.repeat(257), 'line\nbreak', '\u0085'])(
  'rejects invalid retained key %j before credentials or either mutation',
  async (idempotencyKey) => {
    const credential = vi.fn(() => 'unused');
    const fetch = vi.fn<typeof globalThis.fetch>();
    const client = createClient({ baseUrl: 'https://cma.example', credential, fetch });
    const options = { idempotencyKey, onEvent: vi.fn() };
    await expect(
      client.agui.run(
        { threadId: agentId, messages: [{ role: 'user', content: 'Hello' }] },
        options,
      ),
    ).rejects.toBeInstanceOf(ValidationError);
    await expect(client.responses.create(responseRequest, options)).rejects.toBeInstanceOf(
      ValidationError,
    );
    expect(credential).not.toHaveBeenCalled();
    expect(fetch).not.toHaveBeenCalled();
  },
);

it.each(['agui', 'responses'] as const)(
  '%s never automatically resends an uncertain keyed mutation',
  async (protocol) => {
    const fetch = vi.fn<typeof globalThis.fetch>().mockRejectedValue(new Error('reply lost'));
    const client = createClient({ baseUrl: 'https://cma.example', fetch });
    const options = { idempotencyKey: 'retained', onEvent: vi.fn() };
    const promise =
      protocol === 'agui'
        ? client.agui.run(
            { threadId: agentId, messages: [{ role: 'user', content: 'Hello' }] },
            options,
          )
        : client.responses.create(responseRequest, options);
    await expect(promise).rejects.toThrow();
    expect(fetch).toHaveBeenCalledOnce();
    expect(new Headers(fetch.mock.calls[0]![1]!.headers).get('Idempotency-Key')).toBe('retained');
  },
);
