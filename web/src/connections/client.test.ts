import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { connectionClient, type StartedConnection } from './client';

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

it('uses the separate Open endpoint for bounded cached presentation evidence', async () => {
  const report = {
    owner: 'owner',
    ready: true,
    checked_at: '2026-11-16T00:00:00Z',
    services: [],
    verification: 'cached',
    valid_for_ms: 30_000,
  };
  const fetch = vi.fn().mockResolvedValue(Response.json(report));
  vi.stubGlobal('fetch', fetch);
  vi.spyOn(performance, 'now').mockReturnValueOnce(100).mockReturnValueOnce(150);
  expect(await connectionClient.open({ type: 'agent', id: 'agent' })).toEqual({
    ...report,
    valid_for_ms: 29_950,
  });
  expect(fetch.mock.calls[0]?.[0]).toBe('/auth/connections/open');
  expect(fetch).toHaveBeenCalledOnce();
});

it.each([200, 99])(
  'rechecks when response transit exhausts its lifetime or time reverses: %s',
  async (now) => {
    const fresh = { owner: 'owner', ready: true, checked_at: 'now', services: [] };
    vi.spyOn(performance, 'now').mockReturnValueOnce(100).mockReturnValueOnce(now);
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(Response.json({ ...fresh, verification: 'cached', valid_for_ms: 100 }))
      .mockResolvedValueOnce(Response.json(fresh));
    vi.stubGlobal('fetch', fetch);
    expect(await connectionClient.open({ type: 'agent', id: 'agent' })).toEqual(fresh);
    expect(fetch.mock.calls.map(([path]) => path)).toEqual([
      '/auth/connections/open',
      '/auth/connections/check',
    ]);
  },
);

it.each([0, 30_001, undefined])(
  'rechecks cached evidence with invalid lifetime %s freshly',
  async (lifetime) => {
    const report = {
      owner: 'owner',
      ready: true,
      checked_at: '2026-11-16T00:00:00Z',
      services: [],
    };
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(
        Response.json({ ...report, verification: 'cached', valid_for_ms: lifetime }),
      )
      .mockResolvedValueOnce(Response.json(report));
    vi.stubGlobal('fetch', fetch);
    expect(await connectionClient.open({ type: 'agent', id: 'agent' })).toEqual(report);
    expect(fetch.mock.calls.map(([path]) => path)).toEqual([
      '/auth/connections/open',
      '/auth/connections/check',
    ]);
  },
);

it('never accepts a cached response from the fresh verification endpoint', async () => {
  const fetch = vi.fn().mockResolvedValue(Response.json({ ready: true, verification: 'cached' }));
  vi.stubGlobal('fetch', fetch);
  await expect(connectionClient.check({ type: 'agent', id: 'agent' })).rejects.toMatchObject({
    status: 503,
  });
  expect(fetch).toHaveBeenCalledOnce();
});

it.each([401, 403, 429, 503])(
  'does not bypass an Open refusal or throttle %s with another request',
  async (status) => {
    const fetch = vi
      .fn()
      .mockResolvedValue(new Response('{}', { status, headers: { 'retry-after': '5' } }));
    vi.stubGlobal('fetch', fetch);
    await expect(connectionClient.open({ type: 'agent', id: 'agent' })).rejects.toMatchObject({
      status,
    });
    expect(fetch).toHaveBeenCalledOnce();
  },
);

it('falls back only for an older AM missing the Open route', async () => {
  const fetch = vi
    .fn()
    .mockResolvedValueOnce(new Response('', { status: 404 }))
    .mockResolvedValueOnce(Response.json({ ready: true }));
  vi.stubGlobal('fetch', fetch);
  await connectionClient.open({ type: 'agent', id: 'agent' });
  expect(fetch).toHaveBeenCalledTimes(2);
  fetch
    .mockReset()
    .mockResolvedValue(Response.json({ code: 'connection_target_not_found' }, { status: 404 }));
  await expect(connectionClient.open({ type: 'agent', id: 'other' })).rejects.toMatchObject({
    status: 404,
  });
  expect(fetch).toHaveBeenCalledOnce();
});

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
