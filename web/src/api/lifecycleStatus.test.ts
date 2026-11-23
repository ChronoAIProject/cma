import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, createClient, ProtocolError, TransportError } from '@cma/client';
import { createRestWebClient } from './rest';

afterEach(() => vi.unstubAllGlobals());

it.each([401, 403, 404, 429, 503])(
  'preserves HTTP %i for lifecycle retry decisions',
  async (status) => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('', { status })));
    const client = createRestWebClient(createClient({ baseUrl: 'https://cma.test' }));
    await expect(client.readAgentLifecycle!('operation-one')).rejects.toMatchObject({
      name: 'ApiError',
      status,
    });
    await expect(client.readAgentLifecycle!('operation-one')).rejects.toBeInstanceOf(ApiError);
  },
);

it('keeps invalid operation identity a permanent protocol failure', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          id: 'other',
          status: 'succeeded',
          phase: 'agent_startup',
          agent_id: 'other',
          events: [],
        }),
      ),
    ),
  );
  const client = createRestWebClient(createClient({ baseUrl: 'https://cma.test' }));
  await expect(client.readAgentLifecycle!('operation-one')).rejects.toBeInstanceOf(ProtocolError);
});

it('retains a lifecycle refusal and retry floor without exposing upstream detail', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn(
      async () =>
        new Response(
          JSON.stringify({
            status: 503,
            code: 'store_failed',
            retryable: false,
            detail: 'private upstream https://sensitive.example/identity',
          }),
          { status: 503, headers: { 'Retry-After': '3', 'X-Request-Id': 'synthetic-read' } },
        ),
    ),
  );
  const client = createRestWebClient(createClient({ baseUrl: 'https://cma.test' }));
  const error = await client.readAgentLifecycle!('operation-one').catch((error: unknown) => error);
  expect(error).toMatchObject({
    status: 503,
    code: 'store_failed',
    requestId: 'synthetic-read',
    retryAfterMs: 3000,
    extensions: { retryable: false },
  });
  expect((error as Error).message).not.toContain('private upstream');
});

it.each(['null', '[]', '"unexpected"', '{'])(
  'keeps malformed successful lifecycle payload %s a protocol failure',
  async (body) => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => new Response(body)),
    );
    const client = createRestWebClient(createClient({ baseUrl: 'https://cma.test' }));
    await expect(client.readAgentLifecycle!('operation-one')).rejects.toBeInstanceOf(ProtocolError);
  },
);

it.each(['headers', 'body'])(
  'classifies native network failures only at the %s boundary',
  async (phase) => {
    const value = new Response('{}');
    const network = new TypeError('Network unavailable');
    if (phase === 'body') vi.spyOn(value, 'json').mockRejectedValue(network);
    vi.stubGlobal(
      'fetch',
      phase === 'headers' ? vi.fn().mockRejectedValue(network) : vi.fn(async () => value),
    );
    const client = createRestWebClient(createClient({ baseUrl: 'https://cma.test' }));
    await expect(client.readAgentLifecycle!('operation-one')).rejects.toBeInstanceOf(
      TransportError,
    );
  },
);

it('does not retry a malformed successful connection as a network failure', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn(
      async () =>
        new Response(
          JSON.stringify({
            id: 'operation-one',
            status: 'waiting_for_connection',
            phase: 'connect',
            connection: null,
            events: [],
          }),
        ),
    ),
  );
  const client = createRestWebClient(createClient({ baseUrl: 'https://cma.test' }));
  await expect(client.readAgentLifecycle!('operation-one')).rejects.toBeInstanceOf(ProtocolError);
});
