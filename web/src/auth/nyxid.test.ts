import { afterEach, describe, expect, it, vi } from 'vitest';
import { capabilitiesFrom, createNyxidAuthProvider, handleFor } from './nyxid';

/**
 * The identity adapter decides what the whole app believes about who is
 * signed in, so the cases that matter are the ambiguous ones: unreachable,
 * malformed, and authenticated-but-nameless.
 */

afterEach(() => {
  vi.unstubAllGlobals();
});

function stubFetch(impl: (url?: unknown, init?: RequestInit) => Promise<Response> | Response) {
  vi.stubGlobal('fetch', vi.fn(impl));
}

function jsonResponse(status: number, body: unknown): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
  } as Response;
}

describe('handleFor', () => {
  it('prefers the account name the person chose', () => {
    expect(handleFor('76fe9d91', 'Shining Wang', 'shining@aelf.io')).toBe('Shining Wang');
  });

  it('falls back to the local part of an address', () => {
    expect(handleFor('76fe9d91', undefined, 'shining.wang@aelf.io')).toBe('shining.wang');
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
    expect((await createNyxidAuthProvider().current()).status).toBe('anonymous');
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

  it('sends the home page to the Agent Manager', async () => {
    const { replace } = stubLocation('/');
    expect((await anonymous()).signInSilently?.()).toBe(true);
    expect(replace).toHaveBeenCalledWith('/auth/login?return_to=%2Fagent-manager&prompt=none');
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

  it('marks the browser signed out for the control plane to enforce', async () => {
    stubLocation('/agent-manager');
    await createNyxidAuthProvider().signOut();
    expect(document.cookie).toContain('cma_signed_out=1');
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

  it('leaves an address with a query or fragment to the explicit sign-in', async () => {
    for (const [search, hash] of [
      ['?setup=draft', ''],
      ['?tab=access', ''],
      ['', '#turn'],
    ]) {
      const { replace } = stubLocation('/bots/new', search, hash);
      expect((await anonymous()).signInSilently?.(), `${search}${hash}`).toBe(false);
      expect(replace).not.toHaveBeenCalled();
    }
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
