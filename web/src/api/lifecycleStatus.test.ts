import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, createClient, ProtocolError } from '@chronoai/cma-sdk';
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
