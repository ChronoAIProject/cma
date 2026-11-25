import { afterEach, describe, expect, it, vi } from 'vitest';
import { capabilitiesFrom, createNyxidAuthProvider, handleFor, legalTermsFrom } from './nyxid';
import { SIGN_OUT_NOTICE_KEY } from './signOutNotice';

/**
 * The identity adapter decides what the whole app believes about who is
 * signed in, so the cases that matter are the ambiguous ones: unreachable,
 * malformed, and authenticated-but-nameless.
 */

afterEach(() => {
  vi.unstubAllGlobals();
  localStorage.removeItem(SIGN_OUT_NOTICE_KEY);
});

function stubFetch(impl: (url?: unknown, init?: RequestInit) => Promise<Response> | Response) {
  vi.stubGlobal('fetch', vi.fn(impl));
}

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

function opaqueRedirect(): Response {
  const response = new Response();
  Object.defineProperty(response, 'type', { value: 'opaqueredirect' });
  return response;
}

describe('handleFor', () => {
  it('prefers the account name the person chose', () => {
    expect(handleFor('76fe9d91', 'Shining Wang', 'shining@aelf.io')).toBe('Shining Wang');
  });

  it('falls back to the local part of an address', () => {
    expect(handleFor('76fe9d91', undefined, 'user-01@example.com')).toBe('user-01');
  });

  it('shows the identifier only when the provider released nothing', () => {
    // Visible rather than hidden: a raw identifier on screen means the
    // provider gave us no name, which is worth noticing.
    expect(handleFor('76fe9d91')).toBe('76fe9d91');
  });

  it('ignores blank claims', () => {
    expect(handleFor('76fe9d91', '  ', '  ')).toBe('76fe9d91');
  });
});

describe('what the control plane says a caller may do', () => {
  it('keeps the capabilities it recognises', () => {
    expect(capabilitiesFrom(['create_sandbox', 'use_own_sandboxes', 'configure_platform'])).toEqual(
      ['create_sandbox', 'use_own_sandboxes', 'configure_platform'],
    );
  });

  it('drops a name this client does not model', () => {
    // A capability the frontend has no behaviour for is not worth carrying, and
    // keeping the string would invite a comparison that never matches.
    expect(capabilitiesFrom(['read_activity_log', 'read_the_future', 7])).toEqual([
      'read_activity_log',
    ]);
  });

  it('reports none when the field is missing or not a list', () => {
    // Fails closed. The worst outcome is a control that is not offered; the
    // alternative is offering one the control plane will refuse.
    expect(capabilitiesFrom(undefined)).toEqual([]);
    expect(capabilitiesFrom('read_activity_log')).toEqual([]);
    expect(capabilitiesFrom(null)).toEqual([]);
  });
});

describe('the identity adapter', () => {
  it('reports the signed-in subject', async () => {
    stubFetch(() =>
      jsonResponse(200, {
        authenticated: true,
        subject: '76fe9d91',
        display_name: 'Shining Wang',
        email: 'shining@aelf.io',
        avatar_url: 'https://example/a.png',
        capabilities: ['use_own_sandboxes', 'read_activity_log'],
        visitor: true,
      }),
    );
    const state = await createNyxidAuthProvider().current();
    expect(state.status).toBe('authenticated');
    if (state.status === 'authenticated') {
      // The identifier is kept for keying state; the person is shown by name.
      expect(state.user.id).toBe('76fe9d91');
      expect(state.user.handle).toBe('Shining Wang');
      expect(state.user.email).toBe('shining@aelf.io');
      expect(state.user.avatarUrl).toBe('https://example/a.png');
      expect(state.user.capabilities).toEqual(['use_own_sandboxes', 'read_activity_log']);
      expect(state.user.visitor).toBe(true);
    }
  });

  it('gives a caller no capabilities when the control plane names none', async () => {
    // An older control plane that does not report them must leave the client
    // offering nothing privileged, rather than assuming the best.
    stubFetch(() => jsonResponse(200, { authenticated: true, subject: '76fe9d91' }));
    const state = await createNyxidAuthProvider().current();
    if (state.status === 'authenticated') {
      expect(state.user.capabilities).toEqual([]);
    }
  });

  it('reports 401 as signed out', async () => {
    stubFetch(() => jsonResponse(401, { authenticated: false }));
    expect(await createNyxidAuthProvider().current()).toEqual({ status: 'anonymous' });
  });

  it('carries published legal terms signed out and signed in', async () => {
    const wire = {
      version: '2026-11-30',
      terms_url: 'https://legal.example.test/terms',
      privacy_url: 'https://legal.example.test/privacy',
    };
    const legalTerms = {
      version: '2026-11-30',
      termsUrl: 'https://legal.example.test/terms',
      privacyUrl: 'https://legal.example.test/privacy',
    };
    stubFetch(() => jsonResponse(401, { authenticated: false, legal_terms: wire }));
    expect(await createNyxidAuthProvider().current()).toEqual({ status: 'anonymous', legalTerms });
    stubFetch(() =>
      jsonResponse(200, { authenticated: true, subject: '76fe9d91', legal_terms: wire }),
    );
    const state = await createNyxidAuthProvider().current();
    expect(state.status === 'authenticated' && state.user.legalTerms).toEqual(legalTerms);
  });

  it('ignores legal terms it cannot use', () => {
    const valid = {
      version: '1',
      terms_url: 'https://legal.example.test/t',
      privacy_url: 'https://legal.example.test/p',
    };
    expect(legalTermsFrom(valid)).toEqual({
      version: '1',
      termsUrl: 'https://legal.example.test/t',
      privacyUrl: 'https://legal.example.test/p',
    });
    for (const raw of [
      undefined,
      null,
      'terms',
      { ...valid, version: 'two words' },
      { ...valid, terms_url: 'http://legal.example.test/t' },
      { ...valid, privacy_url: 'javascript:alert(1)' },
      { ...valid, terms_url: 42 },
    ])
      expect(legalTermsFrom(raw)).toBeUndefined();
  });

  it('treats an unreachable control plane as an error, not as signed out', async () => {
    // Saying "signed out" here would invite a sign-in that cannot succeed.
    stubFetch(() => {
      throw new Error('network down');
    });
    const state = await createNyxidAuthProvider().current();
    expect(state.status).toBe('error');
  });

  it('treats a server failure as an error', async () => {
    stubFetch(() => jsonResponse(503, {}));
    expect((await createNyxidAuthProvider().current()).status).toBe('error');
  });

  it('treats an unreadable body as an error', async () => {
    stubFetch(
      () =>
        ({
          ok: true,
          status: 200,
          json: async () => {
            throw new Error('not json');
          },
        }) as unknown as Response,
    );
    expect((await createNyxidAuthProvider().current()).status).toBe('error');
  });

  it('treats a 200 that names no subject as signed out', async () => {
    // Never an authenticated identity with a blank subject.
    stubFetch(() => jsonResponse(200, { authenticated: true, subject: '' }));
    expect((await createNyxidAuthProvider().current()).status).toBe('anonymous');

    stubFetch(() => jsonResponse(200, { authenticated: false }));
    expect((await createNyxidAuthProvider().current()).status).toBe('anonymous');
  });

  it('reports a hung control plane rather than waiting forever', async () => {
    // The app renders nothing while this is outstanding, so an unbounded wait
    // is a blank page with no way to act on it — which is what a user sees when
    // the backend is unreachable.
    stubFetch((_url?: unknown, init?: RequestInit) => {
      const err = new Error('aborted');
      err.name = 'AbortError';
      // Mimic the browser: the signal reports itself aborted on timeout.
      Object.defineProperty(init?.signal ?? {}, 'aborted', { value: true, configurable: true });
      throw err;
    });
    const state = await createNyxidAuthProvider().current();
    expect(state.status).toBe('error');
    if (state.status === 'error') {
      expect(state.message).toMatch(/did not respond|cannot reach/i);
    }
  });

  it('never throws, whatever happens', async () => {
    // The contract requires a state rather than an exception, so a failure
    // renders a message instead of a blank surface.
    stubFetch(() => {
      throw new Error('boom');
    });
    await expect(createNyxidAuthProvider().current()).resolves.toBeDefined();
  });
});

describe('silent sign-in', () => {
  function stubLocation(pathname: string, search = '', hash = '') {
    const replace = vi.fn();
    const assign = vi.fn();
    vi.stubGlobal('location', {
      ...window.location,
      origin: 'https://cma.example',
      protocol: 'https:',
      pathname,
      search,
      hash,
      replace,
      assign,
    });
    return { replace, assign };
  }

  /** An adapter that has just been told the browser is signed out. */
  async function anonymous(body: unknown = { authenticated: false, silent_sign_in: true }) {
    stubFetch(() => jsonResponse(401, body));
    const provider = createNyxidAuthProvider();
    expect((await provider.current()).status).toBe('anonymous');
    return provider;
  }

  afterEach(() => {
    vi.unstubAllGlobals();
    sessionStorage.clear();
    document.cookie = 'cma_signed_out=; Path=/; Max-Age=0';
  });

  it('asks the provider without interaction and returns to the opened Agent', async () => {
    const { replace } = stubLocation('/agents/agt_88faf0ed');
    expect((await anonymous()).signInSilently?.()).toBe(true);
    expect(replace).toHaveBeenCalledWith(
      '/auth/login?return_to=%2Fagents%2Fagt_88faf0ed&prompt=none',
    );
  });

  it('returns the home page to its original address', async () => {
    const { replace } = stubLocation('/');
    expect((await anonymous()).signInSilently?.()).toBe(true);
    expect(replace).toHaveBeenCalledWith('/auth/login?return_to=%2F&prompt=none');
  });

  it('is not attempted against a control plane that does not offer it', async () => {
    for (const body of [
      { authenticated: false },
      { authenticated: false, silent_sign_in: false },
      { authenticated: false, silent_sign_in: 'true' },
      'not json',
    ]) {
      const { replace } = stubLocation('/agents/agt_88faf0ed');
      expect((await anonymous(body)).signInSilently?.(), JSON.stringify(body)).toBe(false);
      expect(replace).not.toHaveBeenCalled();
    }
  });

  it('is not attempted before the control plane has been asked', () => {
    const { replace } = stubLocation('/agents/agt_88faf0ed');
    expect(createNyxidAuthProvider().signInSilently?.()).toBe(false);
    expect(replace).not.toHaveBeenCalled();
  });

  it('tries once per tab, so a declined attempt shows the landing', async () => {
    const { replace } = stubLocation('/agents/agt_88faf0ed');
    const provider = await anonymous();
    expect(provider.signInSilently?.()).toBe(true);
    expect(provider.signInSilently?.()).toBe(false);
    expect(replace).toHaveBeenCalledTimes(1);
  });

  it('tries again once this tab has been signed in', async () => {
    const { replace } = stubLocation('/agents/agt_88faf0ed');
    expect((await anonymous()).signInSilently?.()).toBe(true);
    stubFetch(() => jsonResponse(200, { authenticated: true, subject: '76fe9d91' }));
    await createNyxidAuthProvider().current();
    stubLocation('/agents/agt_88faf0ed');
    expect((await anonymous()).signInSilently?.()).toBe(true);
    expect(replace).toHaveBeenCalledTimes(1);
  });

  it('publishes confirmed server logout before navigating and suppresses silent sign-in', async () => {
    const { assign } = stubLocation('/agent-manager');
    const provider = await anonymous();
    stubFetch((url) => {
      expect(localStorage.getItem(SIGN_OUT_NOTICE_KEY)).toBeNull();
      return url === '/auth/logout'
        ? opaqueRedirect()
        : jsonResponse(401, { authenticated: false, silent_sign_in: false });
    });
    await provider.signOut();
    expect(fetch).toHaveBeenCalledWith(
      '/auth/logout',
      expect.objectContaining({ mode: 'same-origin', redirect: 'manual' }),
    );
    expect(localStorage.getItem(SIGN_OUT_NOTICE_KEY)).toMatch(/^[0-9a-f-]{36}$/);
    expect(assign).toHaveBeenCalledWith('/');
    expect(provider.signInSilently?.()).toBe(false);
  });

  it('keeps a failed logout local', async () => {
    const { assign } = stubLocation('/agent-manager');
    const provider = await anonymous();
    stubFetch(() => jsonResponse(503, {}));
    await expect(provider.signOut()).rejects.toThrow('Sign-out did not complete');
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(localStorage.getItem(SIGN_OUT_NOTICE_KEY)).toBeNull();
    expect(assign).not.toHaveBeenCalled();
  });

  it.each([
    [200, { authenticated: true }],
    [401, { authenticated: false, silent_sign_in: true }],
    [401, { code: 'unexpected_response' }],
    [503, { code: 'store_failed' }],
  ])('requires a signed-out session result after a redirect (%s, %j)', async (status, body) => {
    const { assign } = stubLocation('/agent-manager');
    stubFetch((url) => (url === '/auth/logout' ? opaqueRedirect() : jsonResponse(status, body)));
    await expect(createNyxidAuthProvider().signOut()).rejects.toThrow('Sign-out did not complete');
    expect(localStorage.getItem(SIGN_OUT_NOTICE_KEY)).toBeNull();
    expect(assign).not.toHaveBeenCalled();
  });

  it('leaves visitor journeys, share links and pages it cannot return to', async () => {
    for (const pathname of [
      '/t/pub_example',
      '/launches/lch_example',
      '/agent-manager/share/shr_example',
      '/artifacts/doc_example',
      '/connection-return',
      '/agents',
      '/settings/general',
      '/agents/a!',
      '/agents/a%20b',
      `/agents/${'a'.repeat(250)}`,
    ]) {
      const { replace } = stubLocation(pathname);
      expect((await anonymous()).signInSilently?.(), pathname).toBe(false);
      expect(replace, pathname).not.toHaveBeenCalled();
    }
  });

  it('leaves unknown query state and bearer-bearing fragments to explicit sign-in', async () => {
    for (const [search, hash] of [
      ['?next=https://foreign.example', ''],
      ['?setup=x?y&token=authority', ''],
      ['?setup=' + '界'.repeat(9000), ''],
      ['?tab=access', ''],
      ['', '#turn'],
    ]) {
      const { replace } = stubLocation('/bots/new', search, hash);
      expect((await anonymous()).signInSilently?.(), `${search}${hash}`).toBe(false);
      expect(replace).not.toHaveBeenCalled();
    }
  });

  it('preserves setup drafts and Sandbox tabs through the validated return target', async () => {
    for (const [path, search] of [
      ['/bots/new', '?setup=%7B%22name%22%3A%22Draft%22%7D'],
      ['/sandboxes/sbx_1', '?tab=access'],
    ]) {
      sessionStorage.clear();
      const { replace } = stubLocation(path!, search!);
      expect((await anonymous()).signInSilently?.()).toBe(true);
      const target = new URL(replace.mock.calls[0]![0] as string, 'https://cma.example');
      expect(target.searchParams.get('return_to')).toBe(path! + search!);
    }
  });

  it('does not reuse a stale hint after rejection, an outage or malformed identity', async () => {
    for (const [status, body] of [
      [403, { authenticated: false, silent_sign_in: true }],
      [503, { code: 'identity_unavailable' }],
      [200, {}],
      [401, null],
    ] as const) {
      const { replace } = stubLocation('/agent-manager');
      const provider = await anonymous();
      stubFetch(() => jsonResponse(status, body));
      await provider.current();
      expect(provider.signInSilently?.()).toBe(false);
      expect(replace).not.toHaveBeenCalled();
    }
  });

  it('does not loop when the browser refuses navigation', async () => {
    const { replace } = stubLocation('/agent-manager');
    replace.mockImplementation(() => {
      throw new Error('blocked');
    });
    const provider = await anonymous();
    expect(provider.signInSilently?.()).toBe(false);
    expect(provider.signInSilently?.()).toBe(false);
    expect(replace).toHaveBeenCalledTimes(1);
  });

  it('does not try when storage is unavailable, since it could not stop a loop', async () => {
    const { replace } = stubLocation('/agents/agt_88faf0ed');
    const provider = await anonymous();
    vi.stubGlobal('sessionStorage', {
      getItem: () => {
        throw new Error('denied');
      },
    });
    expect(provider.signInSilently?.()).toBe(false);
    expect(replace).not.toHaveBeenCalled();
  });
});

it('recognizes only the explicit identity outage contract and treats refusal as signed out', async () => {
  stubFetch(() => jsonResponse(503, { code: 'identity_unavailable' }));
  expect(await createNyxidAuthProvider().current()).toMatchObject({
    status: 'error',
    reason: 'identity_unavailable',
  });
  stubFetch(() => jsonResponse(503, { code: 'store_failed' }));
  expect(await createNyxidAuthProvider().current()).toMatchObject({
    status: 'error',
    reason: undefined,
  });
  stubFetch(() => jsonResponse(403, {}));
  expect(await createNyxidAuthProvider().current()).toEqual({ status: 'anonymous' });
  stubFetch(() => jsonResponse(429, {}));
  const limited = await createNyxidAuthProvider().current();
  expect(limited).toMatchObject({ status: 'error', reason: undefined });
});
