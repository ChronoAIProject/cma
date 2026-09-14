import { afterEach, expect, it, vi } from 'vitest';
import { connectionClient, type StartedConnection } from './client';

afterEach(() => vi.unstubAllGlobals());

it('polls and cancels a started OAuth handoff using the strict server intent shape', async () => {
  const fetch = vi.fn().mockResolvedValue({ ok: true, json: async () => ({}) });
  vi.stubGlobal('fetch', fetch);
  const target = { type: 'launch' as const, id: 'lch_test' };
  const handoff: StartedConnection = {
    owner: 'owner',
    service_id: 'cma:codex-subscription',
    intent_id: 'intent',
    hosted_url: 'https://auth.openai.com/codex/device',
    expires_at: '2026-09-11T11:00:00Z',
    user_code: 'example-code',
  };
  await connectionClient.check(target, handoff);
  await connectionClient.cancel(target, handoff);
  for (const [index, action] of ['check', 'cancel'].entries()) {
    const [url, options] = fetch.mock.calls[index] as [string, RequestInit];
    expect(url).toBe(`/auth/connections/${action}`);
    expect(JSON.parse(options.body as string)).toEqual({
      target,
      active: { service_id: handoff.service_id, intent_id: handoff.intent_id },
    });
  }
});
