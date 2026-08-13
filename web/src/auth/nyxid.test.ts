import { afterEach, describe, expect, it, vi } from 'vitest';
import { createNyxidAuthProvider, handleFor } from './nyxid';

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

describe('the identity adapter', () => {
  it('reports the signed-in subject', async () => {
    stubFetch(() =>
      jsonResponse(200, {
        authenticated: true,
        subject: '76fe9d91',
        display_name: 'Shining Wang',
        email: 'shining@aelf.io',
        avatar_url: 'https://example/a.png',
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
    // Never an authenticated session with a blank identity.
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
