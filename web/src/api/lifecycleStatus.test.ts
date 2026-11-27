import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, createClient, ProtocolError, TransportError } from '@cma/client';
import { createRestWebClient } from './rest';

afterEach(() => vi.unstubAllGlobals());

it('sends owner cancellation through the public lifecycle endpoint', async () => {
  const fetch = vi.fn(async () =>
    Response.json({ id: 'operation-one', status: 'failed', phase: 'failed', events: [] }),
  );
  const client = createRestWebClient(createClient({ baseUrl: 'https://cma.test', fetch }));
  await expect(client.cancelAgentLifecycle!('operation-one')).resolves.toMatchObject({
    id: 'operation-one',
    status: 'failed',
  });
  expect(fetch).toHaveBeenCalledExactlyOnceWith(
    'https://cma.test/api/v1/agent-lifecycle/operation-one/cancel',
    expect.objectContaining({ method: 'POST' }),
  );
});

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

const repositoryConnection = {
  services: ['api-github'],
  target: { type: 'sandbox_profile', profile: { id: 'profile-one', revision: 1 } },
};
const scopeRepair = {
  connection_id: 'github-one',
  operation: 'public_repository_write',
  required_scope: 'public_repo',
  manage_url: 'https://identity.example/keys/github-one',
};

function readWaitingOperation(connection: unknown) {
  vi.stubGlobal(
    'fetch',
    vi.fn(async () =>
      Response.json({
        id: 'operation-one',
        status: 'waiting_for_connection',
        phase: 'connect',
        events: [],
        connection,
      }),
    ),
  );
  return createRestWebClient(createClient({ baseUrl: 'https://cma.test' })).readAgentLifecycle!(
    'operation-one',
  );
}

it.each([
  undefined,
  'verification_unavailable',
  'missing_connection',
  'insufficient_scope',
  'repository_unavailable',
  'reference_unavailable',
  'runtime_unsupported',
])('retains the repository preflight refusal %s from a saved operation', async (reason) => {
  const connection = {
    ...repositoryConnection,
    ...(reason === undefined ? {} : { repository_access: reason }),
    ...(reason === 'insufficient_scope' ? { repository_scope_repair: scopeRepair } : {}),
  };

  await expect(readWaitingOperation(connection)).resolves.toMatchObject({ connection });
});

it.each([
  { repository_access: 'unknown_refusal' },
  {
    repository_access: 'insufficient_scope',
    repository_scope_repair: { ...scopeRepair, manage_url: null },
  },
])('rejects malformed repository repair data as a protocol failure', async (fields) => {
  await expect(readWaitingOperation({ ...repositoryConnection, ...fields })).rejects.toBeInstanceOf(
    ProtocolError,
  );
});
