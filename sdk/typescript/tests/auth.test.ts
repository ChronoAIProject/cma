import { describe, expect, it, vi } from 'vitest';
import { Authentication } from '../src/auth.js';
import { ApiError, ProtocolError, ValidationError } from '../src/errors.js';
import { readTriggerPreview } from '../src/triggers.js';
import { Launches } from '../src/launch/index.js';
import { previewFixture } from './launch-fixtures.js';
import { transport } from './resource-fixtures.js';

describe('public authentication helpers', () => {
  it('returns authenticated identity with capability authority and optional display fields', async () => {
    const t = transport();
    const body = {
      authenticated: true,
      subject: 'owner-1',
      visitor: false,
      capabilities: ['create_sandbox', 'launch_triggers', 'manage_own_bots'],
      display_name: 'Example owner',
    };
    t.answer(body);
    expect(await t.client.auth.readSession()).toEqual(body);
    expect(new URL(t.calls[0]!.url).pathname).toBe('/auth/session');
    expect(t.calls[0]!.init.redirect).toBe('error');
    expect(t.client.triggers).toBe(t.client.triggerAuthoring);
  });

  it('reads default-off and enabled usage presentation and rejects malformed values', async () => {
    const t = transport();
    const identity = { authenticated: true, subject: 'owner', visitor: false, capabilities: [] };
    for (const measured_visible of [false, true]) {
      t.answer({ ...identity, usage_reporting: { measured_visible } });
      const session = await t.client.auth.readSession();
      expect(session.authenticated && session.usage_reporting?.measured_visible).toBe(
        measured_visible,
      );
    }
    t.answer({ ...identity, usage_reporting: { measured_visible: 'true' } });
    await expect(t.client.auth.readSession()).rejects.toBeInstanceOf(ProtocolError);
    t.answer({ status: 503, code: 'settings_unavailable', detail: 'Settings unavailable.' }, 503);
    await expect(t.client.auth.readSession()).rejects.toMatchObject({
      status: 503,
      code: 'settings_unavailable',
    });
  });

  it('distinguishes signed out, auth-store failure, malformed status, and ordinary problems', async () => {
    const t = transport();
    t.answer({ authenticated: false }, 401);
    expect(await t.client.auth.readSession()).toEqual({ authenticated: false });
    t.answer({ authenticated: false, code: 'store_failed' }, 503);
    await expect(t.client.auth.readSession()).rejects.toMatchObject({
      status: 503,
      code: 'store_failed',
    });
    t.answer({ authenticated: false }, 200);
    await expect(t.client.auth.readSession()).rejects.toBeInstanceOf(ProtocolError);
    t.answer({ status: 429, code: 'rate_limited', detail: 'Read budget exhausted.' }, 429);
    await expect(t.client.auth.readSession()).rejects.toBeInstanceOf(ApiError);
    expect(t.calls).toHaveLength(4);
  });

  it('suppresses credentials and cookies for signup options and both public preview helpers', async () => {
    const t = transport();
    const credential = vi.fn(() => {
      throw new Error('anonymous requests cannot resolve credentials');
    });
    const config = { ...t.client.config, credential, cookies: true };
    t.answer({
      social_providers: ['github'],
      email_signup: false,
      invite_code_required: true,
      register_available: true,
    });
    await new Authentication(config).readSignupOptions();
    t.answer(previewFixture);
    await readTriggerPreview(config, 'pub_example');
    await new Launches(config).preview('pub_example');
    expect(credential).not.toHaveBeenCalled();
    for (const call of t.calls) {
      expect(call.init.credentials).toBe('omit');
      expect(new Headers(call.init.headers).has('authorization')).toBe(false);
    }
  });

  it('preserves middleware problem bodies, correlation and retry metadata on session-store failures', async () => {
    const t = transport();
    const auth = new Authentication({
      ...t.client.config,
      fetch: vi.fn(
        async () =>
          new Response(
            JSON.stringify({
              type: 'https://spec.chrono-ai.fun/cma/1.0/problems/store-failed',
              title: 'store failed',
              status: 503,
              detail: 'Authentication authority is temporarily unavailable.',
              code: 'store_failed',
            }),
            { status: 503, headers: { 'x-request-id': 'contract-request', 'retry-after': '2' } },
          ),
      ),
    });
    await expect(auth.readSession()).rejects.toMatchObject({
      status: 503,
      code: 'store_failed',
      requestId: 'contract-request',
      retryAfterMs: 2000,
    });
  });

  it.each([
    '/',
    '/t/pub-example/plc_1',
    '/triggers/manage',
    '/triggers/manage/trg_1/embed',
    '/agents/agt.example~1',
    '/sandboxes/sbx_1',
    '/profiles',
    '/marketplace',
    '/profiles/agent/agp_1',
  ])('returns a navigation URL for canonical workflow %s without a request', (returnTo) => {
    const t = transport();
    const url = new URL(t.client.auth.loginUrl({ returnTo, screen: 'register' }));
    expect(url.pathname).toBe('/auth/login');
    expect(Object.fromEntries(url.searchParams)).toEqual({
      return_to: returnTo,
      screen: 'register',
    });
    expect(t.client.auth.logoutUrl()).toBe(`${t.client.config.baseUrl}/auth/logout`);
    expect(t.client.auth.loginUrl()).toBe(`${t.client.config.baseUrl}/auth/login`);
    expect(t.fetch).not.toHaveBeenCalled();
  });

  it.each([
    '//attacker.invalid',
    '/agents/..',
    '/t/a/../../',
    '/t/a?code=bad',
    '/t/a#bad',
    '/t/%61',
    '/unknown',
    '/profiles/unknown/id',
    '/t/a\\b',
    '/t/' + 'a'.repeat(256),
  ])('refuses an untrusted return target %s', (returnTo) => {
    const t = transport();
    expect(() => t.client.auth.loginUrl({ returnTo })).toThrow(ValidationError);
    expect(() => t.client.auth.loginUrl({ screen: 'login' as never })).toThrow(ValidationError);
    expect(t.fetch).not.toHaveBeenCalled();
  });
});

it('preserves allowed return queries and rejects the complete forbidden or oversized suffix', () => {
  const t = transport();
  for (const returnTo of ['/bots/new?setup=x?y', '/sandboxes/sbx_1?tab=access']) {
    expect(new URL(t.client.auth.loginUrl({ returnTo })).searchParams.get('return_to')).toBe(
      returnTo,
    );
  }
  for (const returnTo of [
    '/bots/new?setup=x?y&token=authority',
    '/bots/new?setup=' + '界'.repeat(9000),
    '/sandboxes/sbx_1?tab=a&tab=b',
  ]) {
    expect(() => t.client.auth.loginUrl({ returnTo })).toThrow(ValidationError);
  }
  expect(t.calls).toHaveLength(0);
});

it('bounds the fully encoded absolute login URL and rejects raw non-ASCII', async () => {
  const { MAX_LOGIN_URL_BYTES } = await import('../src/auth.js');
  const { readFileSync } = await import('node:fs');
  const budget = JSON.parse(
    readFileSync(
      new URL(
        '../../../api/crates/layer1-public/tests/fixtures/browser-auth-limits.json',
        import.meta.url,
      ),
      'utf8',
    ),
  ) as { max_login_url_bytes: number };
  expect(MAX_LOGIN_URL_BYTES).toBe(budget.max_login_url_bytes);
  const t = transport();
  const base = '/bots/new?setup=';
  const overhead = t.client.auth.loginUrl({ returnTo: base }).length;
  for (const size of [MAX_LOGIN_URL_BYTES - 1, MAX_LOGIN_URL_BYTES]) {
    expect(t.client.auth.loginUrl({ returnTo: base + 'a'.repeat(size - overhead) }).length).toBe(
      size,
    );
  }
  for (const target of [
    base + 'a'.repeat(MAX_LOGIN_URL_BYTES + 1 - overhead),
    base + '%22'.repeat(1500),
    base + '界',
  ]) {
    expect(() => t.client.auth.loginUrl({ returnTo: target })).toThrow(ValidationError);
  }
  expect(t.calls).toHaveLength(0);
});
