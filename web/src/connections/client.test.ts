import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
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

it.each(['{}', 'not-json'])(
  'preserves Retry-After even when the error body is %s',
  async (body) => {
    const fetch = vi.fn().mockResolvedValue(
      new Response(body, {
        status: 429,
        headers: { 'retry-after': '45' },
      }),
    );
    vi.stubGlobal('fetch', fetch);
    const error = await connectionClient
      .check({ type: 'agent', id: 'agt_1' })
      .catch((value: unknown) => value);
    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({ status: 429, retryAfterMs: 45000 });
    expect(fetch).toHaveBeenCalledOnce();
  },
);
