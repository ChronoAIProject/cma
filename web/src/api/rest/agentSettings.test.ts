import { afterEach, expect, it, vi } from 'vitest';
import { agentSettingsClient } from './agentSettings';

afterEach(() => vi.unstubAllGlobals());

it('preserves the settings problem code and Retry-After without exposing upstream detail', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          code: 'agent_delivery_ambiguous',
          detail: '<html>private upstream response</html>',
        }),
        { status: 503, headers: { 'Retry-After': '3' } },
      ),
    ),
  );
  const error = await agentSettingsClient().readAgentSettings!('synthetic-agent').catch(
    (cause: unknown) => cause,
  );
  expect(error).toMatchObject({
    status: 503,
    code: 'agent_delivery_ambiguous',
    retryAfterMs: 3000,
  });
  expect(error).toHaveProperty('message', 'Agent settings could not be loaded or updated.');
});

it('keeps status and retry metadata for a non-JSON settings response', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn().mockResolvedValue(
      new Response('<html>private failure</html>', {
        status: 429,
        headers: { 'Retry-After': '2' },
      }),
    ),
  );
  await expect(agentSettingsClient().readAgentSettings!('synthetic-agent')).rejects.toMatchObject({
    status: 429,
    retryAfterMs: 2000,
    message: 'Agent settings could not be loaded or updated.',
  });
});

const defaults = {
  model: 'gpt-6',
  reasoning_effort: 'high',
  permission_mode: 'autonomous',
} as const;

it('routes settings updates to Agent Manager with command identity and validates the response', async () => {
  const fetch = vi
    .spyOn(globalThis, 'fetch')
    .mockResolvedValue(new Response(JSON.stringify(defaults), { status: 200 }));
  const client = agentSettingsClient();
  await expect(
    client.updateAgentSettings!('agent-one', defaults, { idempotencyKey: 'fixed-key' }),
  ).resolves.toEqual(defaults);
  expect(fetch).toHaveBeenCalledWith(
    '/api/agent-manager/v1/agents/agent-one/settings',
    expect.objectContaining({
      method: 'PATCH',
      headers: {
        'Content-Type': 'application/json',
        'Idempotency-Key': 'fixed-key',
        Prefer: 'cma-settings-reasons',
      },
      body: JSON.stringify(defaults),
    }),
  );
  fetch.mockResolvedValue(
    new Response(JSON.stringify({ ...defaults, permission_mode: 'unknown' }), { status: 200 }),
  );
  await expect(client.readAgentSettings!('agent-one')).rejects.toThrow('invalid Agent settings');
  fetch.mockRestore();
});

it('validates scoped Chrono model responses and preserves same-origin authentication', async () => {
  const fetch = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
    new Response(
      JSON.stringify({ models: [{ id: 'available', label: 'Available', available: true }] }),
      {
        status: 200,
      },
    ),
  );
  try {
    const client = agentSettingsClient();
    await expect(client.readAgentModels!('agent/one')).resolves.toEqual([
      { id: 'available', label: 'Available', available: true },
    ]);
    expect(fetch).toHaveBeenCalledWith(
      '/api/v1/agents/agent%2Fone/models',
      expect.objectContaining({ credentials: 'same-origin', cache: 'no-store' }),
    );
    fetch.mockResolvedValue(
      new Response(
        JSON.stringify({
          models: [
            { id: 'duplicate', label: 'First', available: true },
            { id: 'duplicate', label: 'Second', available: true },
          ],
        }),
        { status: 200 },
      ),
    );
    await expect(client.readAgentModels!('agent-one')).rejects.toThrow('invalid model catalog');
  } finally {
    fetch.mockRestore();
  }
});

it('treats a missing model capability as unavailable and rejects a malformed flag', async () => {
  const fetch = vi.spyOn(globalThis, 'fetch');
  try {
    const client = agentSettingsClient();
    fetch.mockResolvedValue(
      new Response(JSON.stringify({ models: [{ id: 'gpt-6', label: 'GPT-6' }] })),
    );
    await expect(client.readAgentModels!('agent-one')).resolves.toEqual([
      { id: 'gpt-6', label: 'GPT-6', available: false },
    ]);
    fetch.mockResolvedValue(
      new Response(
        JSON.stringify({ models: [{ id: 'gpt-6', label: 'GPT-6', available: 'true' }] }),
      ),
    );
    await expect(client.readAgentModels!('agent-one')).rejects.toThrow('invalid model catalog');
  } finally {
    fetch.mockRestore();
  }
});
