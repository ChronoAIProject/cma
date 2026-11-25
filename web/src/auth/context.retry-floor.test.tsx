import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { AuthContextProvider, useAuth, useIdentityUncertain } from './context';
import { createNyxidAuthProvider } from './nyxid';
import type { AuthProvider, AuthState } from './types';

const signedIn: AuthState = {
  status: 'authenticated',
  user: { id: 'synthetic-owner', handle: 'Fixture', capabilities: [], visitor: false },
};
const outage: AuthState = {
  status: 'error',
  reason: 'identity_unavailable',
  message: 'Temporarily unavailable',
  retry: { kind: 'http', status: 503, retryAfterMs: 90_000 },
};
const response = (status: number, body: unknown, after = '90') =>
  new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json', 'retry-after': after },
  });

function View() {
  const { state, retry, signOut, signIn, sessionReadSignal } = useAuth();
  const uncertain = useIdentityUncertain();
  return (
    <>
      <p>{state.status}</p>
      <p>{uncertain ? 'uncertain' : 'verified'}</p>
      <p>{sessionReadSignal.aborted ? 'reads paused' : 'reads allowed'}</p>
      <button onClick={retry}>Retry</button>
      <button onClick={() => void signOut()}>Sign out</button>
      <button onClick={() => void signIn()}>Sign in</button>
    </>
  );
}

function mount(provider: AuthProvider) {
  return render(
    <AuthContextProvider provider={provider}>
      <View />
    </AuthContextProvider>,
  );
}

function fixture(initial: AuthState = outage) {
  const current = vi.fn<() => Promise<AuthState>>().mockResolvedValue(initial);
  const provider: AuthProvider = { current, signIn: async () => {}, signOut: async () => {} };
  return { provider, current };
}

async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

async function hints() {
  for (const name of ['focus', 'online', 'visibilitychange']) {
    await act(async () => {
      fireEvent(name === 'visibilitychange' ? document : window, new Event(name));
    });
  }
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
  });
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date', 'performance'] });
  vi.setSystemTime(new Date('2026-11-22T00:00:00Z'));
  vi.spyOn(Math, 'random').mockReturnValue(0);
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it.each(['90', 'Sun, 22 Nov 2026 00:01:30 GMT'])(
  'respects the real adapter Retry-After floor %s before verifying recovery',
  async (after) => {
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(
        response(503, { code: 'identity_unavailable', retryable: true }, after),
      )
      .mockImplementation(async () => response(200, { authenticated: true, subject: 'fixture' }));
    vi.stubGlobal('fetch', fetch);
    mount(createNyxidAuthProvider());
    await act(async () => {});
    expect(screen.getByText('uncertain')).toBeInTheDocument();
    await advance(89_999);
    expect(fetch).toHaveBeenCalledTimes(1);
    await advance(1);
    expect(fetch).toHaveBeenCalledTimes(2);
    expect(screen.getByText('verified')).toBeInTheDocument();
    await advance(90_000);
    expect(fetch).toHaveBeenCalledTimes(2);
  },
);

it('coalesces completed-request hints without shortening or extending the monotonic floor', async () => {
  const { provider, current } = fixture();
  mount(provider);
  await act(async () => {});
  await advance(1000);
  vi.setSystemTime(new Date('2036-11-22T00:00:00Z'));
  await hints();
  expect(current).toHaveBeenCalledTimes(1);
  await advance(88_999);
  await hints();
  expect(current).toHaveBeenCalledTimes(1);
  let finish: (state: AuthState) => void = () => {};
  current.mockImplementation(() => new Promise((resolve) => (finish = resolve)));
  await advance(1);
  expect(current).toHaveBeenCalledTimes(2);
  expect(screen.getByText('uncertain')).toBeInTheDocument();
  await hints();
  expect(current).toHaveBeenCalledTimes(2);
  await act(async () => finish(signedIn));
  expect(screen.getByText('verified')).toBeInTheDocument();
});

it('keeps queued refreshes and session reads behind the floor returned by an in-flight check', async () => {
  const { provider, current } = fixture(signedIn);
  mount(provider);
  await act(async () => {});
  expect(screen.getByText('reads allowed')).toBeInTheDocument();

  let finish: (state: AuthState) => void = () => {};
  current.mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)));
  await hints();
  expect(current).toHaveBeenCalledTimes(2);
  expect(screen.getByText('reads paused')).toBeInTheDocument();
  await act(async () => finish(outage));
  expect(screen.getByText('uncertain')).toBeInTheDocument();
  expect(screen.getByText('reads paused')).toBeInTheDocument();
  await advance(89_999);
  expect(current).toHaveBeenCalledTimes(2);
  await advance(1);
  expect(current).toHaveBeenCalledTimes(3);
  expect(screen.getByText('verified')).toBeInTheDocument();
  expect(screen.getByText('reads allowed')).toBeInTheDocument();
  await advance(90_000);
  expect(current).toHaveBeenCalledTimes(3);
});

it.each(['not-a-date', '-1', '0', '2147484', '99999999999999999999'])(
  'keeps the legacy backoff for an invalid Retry-After header %s',
  async (after) => {
    const fetch = vi.fn(async () =>
      response(503, { code: 'identity_unavailable', retryable: true }, after),
    );
    vi.stubGlobal('fetch', fetch);
    mount(createNyxidAuthProvider());
    await act(async () => {});
    await advance(2999);
    expect(fetch).toHaveBeenCalledTimes(1);
    await advance(1);
    expect(fetch).toHaveBeenCalledTimes(2);
  },
);

it.each([
  undefined,
  { kind: 'http', status: 503, retryAfterMs: NaN },
  { kind: 'http', status: 503, retryAfterMs: Infinity },
  { kind: 'http', status: 503, retryAfterMs: -1 },
  { kind: 'http', status: 503, retryAfterMs: 0 },
  { kind: 'http', status: 503, retryAfterMs: 0.5 },
  { kind: 'http', status: 503, retryAfterMs: 2_147_483_648 },
  { kind: 'http', status: 503, retryAfterMs: '90000' },
  { kind: 'http', status: 401, retryAfterMs: 90_000 },
  { kind: 'transport', retryAfterMs: 90_000 },
])(
  'ignores untrusted injected retry metadata %j without changing reason-only recovery',
  async (retry) => {
    const { provider, current } = fixture({ ...outage, retry } as AuthState);
    mount(provider);
    await act(async () => {});
    await advance(2999);
    expect(current).toHaveBeenCalledTimes(1);
    current.mockResolvedValue(signedIn);
    await advance(1);
    expect(current).toHaveBeenCalledTimes(2);
    expect(screen.getByText('verified')).toBeInTheDocument();
  },
);

it('accepts the maximum safe timer floor without overflow or jitter shortening it', async () => {
  vi.spyOn(Math, 'random').mockReturnValue(1);
  const { provider, current } = fixture({
    ...outage,
    retry: { kind: 'http', status: 503, retryAfterMs: 2_147_483_647 },
  } as AuthState);
  mount(provider);
  await act(async () => {});
  await advance(2_147_483_646);
  expect(current).toHaveBeenCalledTimes(1);
  current.mockResolvedValue(signedIn);
  await advance(1);
  expect(current).toHaveBeenCalledTimes(2);
});

it.each([
  [401, { authenticated: false, retryable: true }],
  [403, { authenticated: false, retryable: true }],
  [503, { code: 'store_failed', retryable: false }],
  [503, { code: 'store_failed' }],
  [429, { retryable: true }],
])(
  'does not introduce automatic retries for HTTP %s without a recovery reason',
  async (status, body) => {
    const fetch = vi.fn(async () => response(status as number, body));
    vi.stubGlobal('fetch', fetch);
    mount(createNyxidAuthProvider());
    await act(async () => {});
    await advance(180_000);
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(screen.getByText('uncertain')).toBeInTheDocument();
  },
);

it('keeps malformed identity bodies unverified without adding automatic retry', async () => {
  const fetch = vi.fn(async () => new Response('{', { status: 200 }));
  vi.stubGlobal('fetch', fetch);
  mount(createNyxidAuthProvider());
  await act(async () => {});
  await advance(180_000);
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(screen.getByText('uncertain')).toBeInTheDocument();
});

it('gates an explicit retry for typed 429 without automatically retrying the initial error', async () => {
  const fetch = vi
    .fn()
    .mockResolvedValueOnce(response(429, { retryable: true }))
    .mockImplementation(async () => response(200, { authenticated: true, subject: 'fixture' }));
  vi.stubGlobal('fetch', fetch);
  mount(createNyxidAuthProvider());
  await act(async () => {});
  await hints();
  expect(fetch).toHaveBeenCalledTimes(1);
  await advance(89_999);
  expect(fetch).toHaveBeenCalledTimes(1);
  await advance(1);
  expect(fetch).toHaveBeenCalledTimes(2);
});

it('clears the floor on success so a later explicit check is not held back', async () => {
  const { provider, current } = fixture();
  mount(provider);
  await act(async () => {});
  current.mockResolvedValue(signedIn);
  await advance(90_000);
  expect(current).toHaveBeenCalledTimes(2);
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
  });
  expect(current).toHaveBeenCalledTimes(3);
});

it('cancels the old timer and suppresses refresh hints while sign-out is pending', async () => {
  const { provider, current } = fixture();
  let finish = () => {};
  provider.signOut = () => new Promise<void>((resolve) => (finish = resolve));
  mount(provider);
  await act(async () => {});
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Sign out' }));
  });
  await hints();
  await advance(90_000);
  expect(current).toHaveBeenCalledTimes(1);
  await act(async () => finish());
  expect(screen.getByText('anonymous')).toBeInTheDocument();
  await advance(90_000);
  expect(current).toHaveBeenCalledTimes(1);
  current.mockResolvedValue(signedIn);
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
  });
  expect(current).toHaveBeenCalledTimes(1);
  expect(screen.getByText('anonymous')).toBeInTheDocument();
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }));
  });
  expect(current).toHaveBeenCalledTimes(2);
  expect(screen.getByText('verified')).toBeInTheDocument();
});

it('does not restore a timer from a pre-sign-out response that arrives late', async () => {
  const { provider, current } = fixture();
  let finish: (state: AuthState) => void = () => {};
  current.mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)));
  mount(provider);
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Sign out' }));
  });
  await act(async () => finish(outage));
  await advance(180_000);
  expect(current).toHaveBeenCalledTimes(1);
  expect(screen.getByText('anonymous')).toBeInTheDocument();
});

it('clears the old floor on sign-in and retains the new identity display during later recovery', async () => {
  const { provider, current } = fixture();
  mount(provider);
  await act(async () => {});
  current.mockResolvedValue(signedIn);
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }));
  });
  expect(current).toHaveBeenCalledTimes(2);
  current.mockResolvedValue(outage);
  await act(async () => {
    fireEvent(window, new Event('focus'));
  });
  expect(current).toHaveBeenCalledTimes(3);
  expect(screen.getByText('authenticated')).toBeInTheDocument();
  expect(screen.getByText('uncertain')).toBeInTheDocument();
  await advance(89_999);
  expect(current).toHaveBeenCalledTimes(3);
});

it('respects a fresh floor returned by the identity check after explicit sign-in', async () => {
  const { provider, current } = fixture({ status: 'anonymous' });
  mount(provider);
  await act(async () => {});
  current.mockResolvedValue(outage);
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }));
  });
  expect(current).toHaveBeenCalledTimes(2);
  await advance(1000);
  await hints();
  await advance(88_999);
  expect(current).toHaveBeenCalledTimes(2);
  current.mockResolvedValue(signedIn);
  await advance(1);
  expect(current).toHaveBeenCalledTimes(3);
  expect(screen.getByText('verified')).toBeInTheDocument();
});

it.each([false, true])('cleans up an old provider with pending response %s', async (pending) => {
  const old = fixture();
  let finish: (state: AuthState) => void = () => {};
  if (pending)
    old.current.mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)));
  const view = mount(old.provider);
  await act(async () => {});
  const fresh = fixture(signedIn);
  view.rerender(
    <AuthContextProvider provider={fresh.provider}>
      <View />
    </AuthContextProvider>,
  );
  await act(async () => finish(outage));
  await advance(180_000);
  expect(old.current).toHaveBeenCalledTimes(1);
  expect(fresh.current).toHaveBeenCalledTimes(1);
  expect(screen.getByText('verified')).toBeInTheDocument();
});

it('does not retain the previous provider identity through a new provider outage', async () => {
  const old = fixture(signedIn);
  const view = mount(old.provider);
  await act(async () => {});
  const fresh = fixture();
  view.rerender(
    <AuthContextProvider provider={fresh.provider}>
      <View />
    </AuthContextProvider>,
  );
  await act(async () => {});
  expect(screen.getByText('error')).toBeInTheDocument();
  expect(screen.getByText('uncertain')).toBeInTheDocument();
});

it('discards a sign-in that completes after its provider was replaced', async () => {
  const old = fixture();
  let finish = () => {};
  old.provider.signIn = () => new Promise<void>((resolve) => (finish = resolve));
  const view = mount(old.provider);
  await act(async () => {});
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }));
  });
  const fresh = fixture(signedIn);
  view.rerender(
    <AuthContextProvider provider={fresh.provider}>
      <View />
    </AuthContextProvider>,
  );
  await act(async () => finish());
  await advance(180_000);
  expect(old.current).toHaveBeenCalledTimes(1);
  expect(fresh.current).toHaveBeenCalledTimes(1);
  expect(screen.getByText('verified')).toBeInTheDocument();
});

it.each([false, true])('cleans up on unmount with pending response %s', async (pending) => {
  const { provider, current } = fixture();
  let finish: (state: AuthState) => void = () => {};
  if (pending) current.mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)));
  const view = mount(provider);
  await act(async () => {});
  view.unmount();
  await act(async () => finish(outage));
  await advance(180_000);
  expect(current).toHaveBeenCalledTimes(1);
});
