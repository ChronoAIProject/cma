import { afterEach, expect, it, vi } from 'vitest';
import { agentPreferencesClient } from './agentPreferences';
const view = {
  system_prompt: 'Owner instructions',
  prompt_pending: false,
  prompt_supported: true,
  revision: 7,
};
afterEach(() => vi.unstubAllGlobals());
it('uses the public Agent preferences route and preserves retry identity and editable prompt exactly', async () => {
  const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify(view)));
  vi.stubGlobal('fetch', fetcher);
  const client = agentPreferencesClient();
  const update = { expected_revision: 7, system_prompt: '  precise\nowner instructions  ' };
  await expect(
    client.updateAgentPreferences?.('agent/id', update, { idempotencyKey: 'same-request' }),
  ).resolves.toEqual(view);
  expect(fetcher).toHaveBeenCalledWith(
    '/api/v1/agents/agent%2Fid/preferences',
    expect.objectContaining({
      method: 'PATCH',
      cache: 'no-store',
      credentials: 'same-origin',
      body: JSON.stringify(update),
      headers: { 'Content-Type': 'application/json', 'Idempotency-Key': 'same-request' },
    }),
  );
});
it('does not represent absent runtime confirmation as saved preferences', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn().mockResolvedValue(new Response(JSON.stringify({ ...view, prompt_pending: 'yes' }))),
  );
  await expect(agentPreferencesClient().readAgentPreferences?.('agent')).rejects.toThrow(
    'invalid Agent preferences',
  );
});
it('preserves a conflict status for revision-aware editing', async () => {
  vi.stubGlobal(
    'fetch',
    vi
      .fn()
      .mockResolvedValue(
        new Response(JSON.stringify({ detail: 'Revision changed' }), { status: 409 }),
      ),
  );
  await expect(
    agentPreferencesClient().updateAgentPreferences?.(
      'agent',
      { expected_revision: 7, title: 'Name' },
      { idempotencyKey: 'rename' },
    ),
  ).rejects.toMatchObject({ status: 409, message: 'Revision changed' });
});
