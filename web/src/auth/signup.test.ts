import { afterEach, expect, it, vi } from 'vitest';
import { readSignupOptions } from './signup';
import { signInPath } from './nyxid';

afterEach(() => vi.unstubAllGlobals());

it('reads public signup options without inferring omitted fields', async () => {
  const fetch = vi.fn(
    async () =>
      new Response(
        JSON.stringify({
          social_providers: ['github', 'google'],
          email_signup: true,
          invite_code_required: false,
          register_available: true,
        }),
      ),
  );
  vi.stubGlobal('fetch', fetch);
  await expect(readSignupOptions()).resolves.toEqual({
    socialProviders: ['github', 'google'],
    emailSignup: true,
    inviteCodeRequired: false,
    registerAvailable: true,
  });
  expect(fetch).toHaveBeenCalledWith(
    '/auth/signup-options',
    expect.objectContaining({ credentials: 'include', signal: expect.any(AbortSignal) }),
  );
});

it.each([
  null,
  {},
  {
    social_providers: [1],
    email_signup: true,
    invite_code_required: false,
    register_available: true,
  },
  {
    social_providers: [],
    email_signup: true,
    invite_code_required: false,
    register_available: 'yes',
  },
])('rejects malformed public account options %j', async (body) => {
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => new Response(JSON.stringify(body))),
  );
  await expect(readSignupOptions()).rejects.toThrow('Account options are unavailable');
});

it('keeps unavailable configuration distinct from closed registration', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => new Response('{}', { status: 503 })),
  );
  await expect(readSignupOptions()).rejects.toThrow();
});

it('builds canonical local sign-in and registration return routes', () => {
  expect(signInPath()).toBe('/auth/login');
  expect(signInPath('/t/public-example/plc_readme')).toBe(
    '/auth/login?return_to=%2Ft%2Fpublic-example%2Fplc_readme',
  );
  expect(signInPath('/t/public-example', { screen: 'register' })).toBe(
    '/auth/login?return_to=%2Ft%2Fpublic-example&screen=register',
  );
  expect(signInPath('/profiles/agent-profiles/agp_example')).toContain('return_to=');
});

it.each([
  'https://other.example.test',
  '//other.example.test',
  '/t/example?prompt=private',
  '/t/%2e%2e',
  '/t/../x',
  '/profiles/agent/id',
  '/unknown',
])('refuses noncanonical sign-in target %s', (target) => {
  expect(() => signInPath(target)).toThrow();
});
