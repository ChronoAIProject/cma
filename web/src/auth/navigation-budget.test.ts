import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, expect, it, vi } from 'vitest';
import { MAX_LOGIN_URL_BYTES } from '../../../sdk/typescript/src/auth';
import { createNyxidAuthProvider, signInPath, silentReturnTarget } from './nyxid';
import { readFatherDraft } from '../bots/fatherDraft';

const budget = JSON.parse(
  readFileSync(
    resolve(
      dirname(fileURLToPath(import.meta.url)),
      '../../../api/crates/layer1-public/tests/fixtures/browser-auth-limits.json',
    ),
    'utf8',
  ),
) as { max_login_url_bytes: number };
const origin = 'https://cma.example';
function location(pathname: string, search = '') {
  const assign = vi.fn(),
    replace = vi.fn();
  vi.stubGlobal('location', {
    origin,
    pathname,
    search,
    hash: '',
    protocol: 'https:',
    assign,
    replace,
  });
  return { assign, replace };
}
afterEach(() => {
  vi.unstubAllGlobals();
  sessionStorage.clear();
});
function targetAt(bytes: number, prompt = false) {
  let value = '/bots/new?setup=';
  const url = new URL('/auth/login', origin);
  url.searchParams.set('return_to', value);
  if (prompt) url.searchParams.set('prompt', 'none');
  value += 'a'.repeat(bytes - url.href.length);
  return value;
}
it('uses the shared encoded absolute login URL budget at the exact boundary', () => {
  expect(MAX_LOGIN_URL_BYTES).toBe(budget.max_login_url_bytes);
  location('/bots/new');
  for (const size of [MAX_LOGIN_URL_BYTES - 1, MAX_LOGIN_URL_BYTES]) {
    expect(new URL(signInPath(targetAt(size)), origin).href.length).toBe(size);
  }
  expect(signInPath(targetAt(MAX_LOGIN_URL_BYTES + 1))).toBe('/auth/login?return_to=%2Fbots%2Fnew');
  const silent = targetAt(MAX_LOGIN_URL_BYTES, true).slice('/bots/new'.length);
  expect(silentReturnTarget({ pathname: '/bots/new', search: silent, hash: '' })).toBe(
    '/bots/new' + silent,
  );
  expect(
    silentReturnTarget({ pathname: '/bots/new', search: silent + 'a', hash: '' }),
  ).toBeUndefined();
});
it('keeps a maximal encoded draft in its tab and falls back before any 414 request', async () => {
  const draft = { v: 1, name: 'n'.repeat(80), instructions: ' '.repeat(4095) + 'x' };
  const search = '?setup=' + encodeURIComponent(JSON.stringify(draft));
  const nav = location('/bots/new', search);
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => ({
      status: 401,
      ok: false,
      json: async () => ({ authenticated: false, silent_sign_in: true }),
    })),
  );
  const auth = createNyxidAuthProvider();
  await auth.current();
  expect(auth.signInSilently?.()).toBe(false);
  expect(nav.replace).not.toHaveBeenCalled();
  expect(window.location.search).toBe(search);
  expect(JSON.parse(sessionStorage.getItem('cma:bot-father-draft:v1')!)).toEqual(draft);
  await auth.signIn('/bots/new' + window.location.search, { screen: 'register' });
  expect(nav.assign).toHaveBeenCalledWith('/auth/login?return_to=%2Fbots%2Fnew&screen=register');
  expect(readFatherDraft()).toEqual(draft);
  expect(JSON.parse(sessionStorage.getItem('cma:bot-father-draft:v1')!)).toEqual(draft);
  expect(window.location.search).toContain('setup=');
});
it.each(['/bots/new?setup=界', '/sandboxes/a?tab=é', '/bots/new?setup=x?ignored=1&token=secret'])(
  'rejects noncanonical input %s',
  (target) => {
    location('/');
    expect(() => signInPath(target)).toThrow();
  },
);
