import { describe, expect, it, vi } from 'vitest';
import { createClient } from '../src/client.js';
import { ApiError, ProtocolError } from '../src/errors.js';
import { transport } from './resource-fixtures.js';

describe('explicit unsupported operations', () => {
  it('preserves named problems and identity/capability extensions without retrying', async () => {
    const test = transport();
    for (const [invoke, path, extension] of [
      [
        () => test.client.agents.terminal('agent/id'),
        '/api/v1/agents/agent%2Fid/terminal',
        { capability: 'terminal' },
      ],
      [() => test.client.unsupported.mcp(), '/api/v1/mcp', { interface: 'mcp' }],
      [() => test.client.unsupported.a2a(), '/api/v1/a2a', { interface: 'a2a' }],
      [
        () => test.client.unsupported.agentCard(),
        '/.well-known/agent-card.json',
        { interface: 'a2a' },
      ],
    ] as const) {
      const code = 'capability' in extension ? 'capability_unsupported' : 'interface_not_available';
      test.answer({ status: 501, code, detail: 'Unavailable', ...extension }, 501);
      await expect(invoke()).rejects.toMatchObject({ status: 501, code, extensions: extension });
      expect(new URL(test.calls.at(-1)!.url).pathname).toBe(path);
      expect(test.calls.at(-1)!.init.body).toBeUndefined();
    }
    expect(test.calls).toHaveLength(4);
    expect(test.calls.map(({ init }) => init.method)).toEqual(['GET', 'POST', 'POST', 'GET']);
    test.answer({ status: 404, code: 'agent_not_found', detail: 'Not found' }, 404);
    await expect(test.client.agents.terminal('other-owner')).rejects.toBeInstanceOf(ApiError);
  });

  it('does not turn an unexpected successful response into a supported capability', async () => {
    const test = transport();
    test.answer({ attachment: 'unexpected' });
    await expect(test.client.agents.terminal('agent-id')).rejects.toBeInstanceOf(ProtocolError);
    test.answer(undefined, 204);
    await expect(test.client.unsupported.mcp()).rejects.toBeInstanceOf(ProtocolError);
  });

  it('reads the public Agent card refusal without resolving credentials', async () => {
    const credential = vi.fn(() => {
      throw new Error('must not resolve credentials');
    });
    const fetch = vi.fn(
      async () =>
        new Response(
          JSON.stringify({
            status: 501,
            code: 'interface_not_available',
            detail: 'Unavailable',
            interface: 'a2a',
          }),
          { status: 501 },
        ),
    );
    const client = createClient({ credential, cookies: true, fetch });
    await expect(client.unsupported.agentCard()).rejects.toBeInstanceOf(ApiError);
    expect(credential).not.toHaveBeenCalled();
    expect(fetch).toHaveBeenCalledWith(
      expect.any(String),
      expect.objectContaining({ credentials: 'omit' }),
    );
  });
});

it('reads typed provider discovery anonymously through both entry points', async () => {
  const card = {
    name: 'CMA Codex provider',
    version: '0.1.0',
    url: 'https://cma.example',
    capabilities: {
      streaming: true,
      activity_items: true,
      client_tools: false,
      cancel: true,
      rich_input: false,
    },
    provider_contract: {
      binding: '/api/v1/cmaeg/bindings/{binding_id}',
      conversation: '/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}',
      responses: '/api/v2/responses',
    },
    authentication: { schemes: ['bearer-agent-key'] },
  };
  const credential = vi.fn(() => {
    throw new Error('public discovery must not resolve credentials');
  });
  const fetch = vi.fn(async () => new Response(JSON.stringify(card)));
  const client = createClient({ credential, cookies: true, fetch });
  expect(await client.readAgentCard()).toEqual(card);
  expect(await client.unsupported.agentCard()).toEqual(card);
  expect(credential).not.toHaveBeenCalled();
  expect(fetch.mock.calls).toHaveLength(2);
});
