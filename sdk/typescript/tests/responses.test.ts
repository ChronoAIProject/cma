import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { Cma } from '../src/client.js';
import { resolveConfig } from '../src/config.js';
import { ProtocolError, RequestAborted } from '../src/errors.js';
import {
  createResponse,
  parseResponsesEventStream,
  toConversation,
  toConversationItemList,
  toResponse,
  type ResponsesResponse,
} from '../src/responses.js';

/** @vitest-environment node */

const SERVER_FIXTURES = new URL(
  '../../../api/crates/layer1-public/tests/fixtures/responses-sdk/',
  import.meta.url,
);

function fixtureText(name: string): string {
  return readFileSync(new URL(name, SERVER_FIXTURES), 'utf8');
}

function fixture(name: string): unknown {
  return JSON.parse(fixtureText(name)) as unknown;
}

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  });
}

function streamResponse(body: string): Response {
  return new Response(body, {
    status: 200,
    headers: { 'content-type': 'text/event-stream; charset=utf-8' },
  });
}

function responseConfig(fetch: typeof globalThis.fetch) {
  return resolveConfig({ fetch, requestIdProvider: () => 'responses-sdk-test' });
}

describe('server-owned Responses fixtures', () => {
  it('decodes conversation, item, response, and stream shapes', () => {
    expect(toConversation(fixture('conversation.json'))).toMatchObject({
      id: 'sesn_fixture',
      metadata: { title: 'Fixture chat' },
    });
    expect(toConversationItemList(fixture('items.json')).data.map((item) => item.type)).toEqual([
      'message',
      'cma_chat.tool_activity',
      'message',
    ]);
    expect(toResponse(fixture('response.json'))).toMatchObject({
      id: 'resp_sesn_fixture_1',
      model: 'chat-model-fixture',
      status: 'completed',
      usage: { total_tokens: 16 },
    });

    const events = parseResponsesEventStream(fixtureText('projection.sse'));
    expect(events.map((event) => event.sequence_number)).toEqual(
      Array.from({ length: 16 }, (_unused, index) => index),
    );
    expect(events.at(-1)?.type).toBe('response.completed');
  });

  it('decodes partial assistant output while a response is still in progress', () => {
    const partial = structuredClone(fixture('response.json')) as Record<string, unknown>;
    partial.status = 'in_progress';
    partial.usage = null;
    const message = (partial.output as Record<string, unknown>[])[0];
    if (message === undefined) {
      throw new Error('response fixture output is empty');
    }
    message.status = 'in_progress';

    expect(toResponse(partial)).toMatchObject({
      status: 'in_progress',
      output: [{ status: 'in_progress', content: [{ text: 'Hello from CMA' }] }],
    });
  });
});

describe('Responses resources', () => {
  it('creates and reads conversations, lists items, and retrieves responses', async () => {
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(fixture('conversation.json')))
      .mockResolvedValueOnce(jsonResponse(fixture('conversation.json')))
      .mockResolvedValueOnce(jsonResponse(fixture('items.json')))
      .mockResolvedValueOnce(jsonResponse(fixture('response.json')));
    const client = new Cma({
      fetch: fetch as unknown as typeof globalThis.fetch,
      requestIdProvider: () => 'responses-resources-test',
    });

    await expect(client.createConversation()).resolves.toMatchObject({ id: 'sesn_fixture' });
    await expect(client.readConversation('sesn/fixture')).resolves.toMatchObject({
      id: 'sesn_fixture',
    });
    await expect(
      client.listConversationItems('sesn/fixture', { after: 'msg/next', limit: 7 }),
    ).resolves.toMatchObject({ first_id: 'msg_sesn_fixture_1', has_more: false });
    await expect(client.readResponse('resp/sesn/fixture/1')).resolves.toMatchObject({
      model: 'chat-model-fixture',
    });

    const calls = fetch.mock.calls as unknown as [string, RequestInit][];
    expect(calls.map(([url]) => url)).toEqual([
      '/api/v1/conversations',
      '/api/v1/conversations/sesn%2Ffixture',
      '/api/v1/conversations/sesn%2Ffixture/items?after=msg%2Fnext&limit=7',
      '/api/v1/responses/resp%2Fsesn%2Ffixture%2F1',
    ]);
    expect(calls[0]?.[1]).toMatchObject({ method: 'POST', body: '{}' });
  });

  it('rejects invalid item pagination before fetch', async () => {
    const fetch = vi.fn();
    const client = new Cma({ fetch: fetch as unknown as typeof globalThis.fetch });
    await expect(client.listConversationItems('sesn_fixture', { limit: 0 })).rejects.toThrow(
      'from 1 through 100',
    );
    await expect(client.listConversationItems('sesn_fixture', { after: '' })).rejects.toThrow(
      'must not be empty',
    );
    expect(fetch).not.toHaveBeenCalled();
  });
});

describe('ResponsesConnection', () => {
  it('folds text, live reasoning, current tool state, and configured-model completion', async () => {
    const fetch = vi.fn(() => Promise.resolve(streamResponse(fixtureText('projection.sse'))));
    const connection = await createResponse(
      responseConfig(fetch as unknown as typeof globalThis.fetch),
      {
        conversation: 'sesn_fixture',
        input: 'Open the contract',
        metadata: { trace: 'sdk-fixture' },
        model: 'caller-model-is-ignored',
      },
    );
    const state = await connection.fold();

    expect(state).toMatchObject({
      outputText: 'Hello from CMA',
      reasoningText: 'checking the contract',
      terminal: {
        id: 'resp_sesn_fixture_1',
        model: 'chat-model-fixture',
        status: 'completed',
      },
    });
    expect(state.toolActivities).toEqual([
      expect.objectContaining({ call_id: 'call_fixture', status: 'completed', duration_ms: 1000 }),
    ]);

    const [[url, init]] = fetch.mock.calls as unknown as [[string, RequestInit]];
    expect(url).toBe('/api/v1/responses');
    expect(init.method).toBe('POST');
    expect(JSON.parse(String(init.body))).toEqual({
      conversation: 'sesn_fixture',
      input: 'Open the contract',
      metadata: { trace: 'sdk-fixture' },
      model: 'caller-model-is-ignored',
      stream: true,
    });
  });

  it.each(['completed', 'failed', 'incomplete'] as const)(
    'folds a %s terminal event',
    async (status) => {
      const response = structuredClone(fixture('response.json')) as ResponsesResponse;
      const terminal: ResponsesResponse = {
        ...response,
        status,
        error:
          status === 'failed' ? { code: 'fixture_failure', message: 'The fixture failed.' } : null,
        incomplete_details: status === 'incomplete' ? { reason: 'max_output_tokens' } : null,
      };
      const type = `response.${status}`;
      const body = `event: ${type}\ndata: ${JSON.stringify({
        type,
        sequence_number: 0,
        response: terminal,
      })}\n\n`;
      const fetch = vi.fn(() => Promise.resolve(streamResponse(body)));
      const connection = await createResponse(
        responseConfig(fetch as unknown as typeof globalThis.fetch),
        { conversation: 'sesn_fixture', input: 'continue' },
      );
      await expect(connection.fold()).resolves.toMatchObject({
        outputText: 'Hello from CMA',
        terminal: { status },
      });
    },
  );

  it('rejects malformed JSON and missing or non-contiguous sequence numbers', () => {
    expect(() =>
      parseResponsesEventStream('event: response.created\ndata: {not json}\n\n'),
    ).toThrow(ProtocolError);
    expect(() =>
      parseResponsesEventStream(
        'event: response.output_item.added\ndata: {"type":"response.output_item.added"}\n\n',
      ),
    ).toThrow(/sequence_number/);
    expect(() =>
      parseResponsesEventStream(
        [
          'event: response.output_item.added',
          'data: {"type":"response.output_item.added","sequence_number":0}',
          '',
          'event: response.output_item.done',
          'data: {"type":"response.output_item.done","sequence_number":2}',
          '',
        ].join('\n') + '\n',
      ),
    ).toThrow('expected sequence 1 but received 2');
  });

  it('cancels only the local reader and reports the caller-visible abort', async () => {
    const encoded = new TextEncoder().encode(
      `event: response.created\ndata: ${JSON.stringify({
        type: 'response.created',
        sequence_number: 0,
        response: {
          ...(fixture('response.json') as object),
          status: 'in_progress',
          output: [],
          usage: null,
        },
      })}\n\n`,
    );
    const readerCancelled = vi.fn();
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(encoded);
      },
      cancel: readerCancelled,
    });
    const fetch = vi.fn(() =>
      Promise.resolve(
        new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } }),
      ),
    );
    const connection = await createResponse(
      responseConfig(fetch as unknown as typeof globalThis.fetch),
      { conversation: 'sesn_fixture', input: 'continue' },
    );
    const iterator = connection[Symbol.asyncIterator]();
    await expect(iterator.next()).resolves.toMatchObject({
      done: false,
      value: { type: 'response.created' },
    });
    connection.cancel('window closed');
    await expect(iterator.next()).rejects.toBeInstanceOf(RequestAborted);
    expect(readerCancelled).toHaveBeenCalled();
    expect(fetch).toHaveBeenCalledTimes(1);
    expect((fetch.mock.calls[0] as unknown as [string])[0]).toBe('/api/v1/responses');
  });

  it('dropping iteration closes the body but never calls the server cancellation route', async () => {
    const encoded = new TextEncoder().encode(
      `event: response.created\ndata: ${JSON.stringify({
        type: 'response.created',
        sequence_number: 0,
        response: {
          ...(fixture('response.json') as object),
          status: 'in_progress',
          output: [],
          usage: null,
        },
      })}\n\n`,
    );
    const readerCancelled = vi.fn();
    const fetch = vi.fn(() =>
      Promise.resolve(
        new Response(
          new ReadableStream<Uint8Array>({
            start(controller) {
              controller.enqueue(encoded);
            },
            cancel: readerCancelled,
          }),
          { status: 200, headers: { 'content-type': 'text/event-stream' } },
        ),
      ),
    );
    const connection = await createResponse(
      responseConfig(fetch as unknown as typeof globalThis.fetch),
      { conversation: 'sesn_fixture', input: 'continue' },
    );
    for await (const event of connection) {
      expect(event.type).toBe('response.created');
      break;
    }

    expect(readerCancelled).toHaveBeenCalled();
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(
      (fetch.mock.calls as unknown as [string, RequestInit][]).some(([url]) =>
        url.endsWith('/cancel'),
      ),
    ).toBe(false);
  });
});
