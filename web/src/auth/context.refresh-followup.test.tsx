import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { AuthContextProvider, useAuth } from './context';
import { SIGN_OUT_NOTICE_KEY } from './signOutNotice';
import type { AuthProvider, AuthState } from './types';

const owner = (id: string): AuthState => ({
  status: 'authenticated',
  user: { id, handle: id, capabilities: [], visitor: false },
});

function deferred() {
  let resolve!: (state: AuthState) => void;
  let reject!: (cause: unknown) => void;
  const promise = new Promise<AuthState>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

function View() {
  const { state, retry, signIn, signOut, explicitlySignedOut, sessionReadSignal } = useAuth();
  return (
    <>
      <p>{state.status === 'authenticated' ? state.user.id : state.status}</p>
      <output aria-label="sign-out notice">{explicitlySignedOut ? 'received' : 'none'}</output>
      <output aria-label="session reads">{sessionReadSignal.aborted ? 'paused' : 'allowed'}</output>
      <button onClick={retry}>Retry</button>
      <button onClick={() => void signIn()}>Sign in</button>
      <button onClick={() => void signOut()}>Sign out</button>
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

async function fixture() {
  const current = vi.fn<AuthProvider['current']>().mockResolvedValue(owner('owner-a'));
  const provider: AuthProvider = { current, signIn: vi.fn(), signOut: vi.fn() };
  const view = mount(provider);
  await act(async () => {});
  const held = deferred();
  current.mockReturnValueOnce(held.promise);
  await hint('focus');
  expect(current).toHaveBeenCalledTimes(2);
  return { current, provider, view, held };
}

async function hint(event: string) {
  await act(async () => {
    if (event === 'retry') fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    else fireEvent(event === 'visibilitychange' ? document : window, new Event(event));
  });
}

async function advance(ms: number) {
  await act(async () => vi.advanceTimersByTimeAsync(ms));
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
  vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
});
afterEach(() => {
  window.localStorage.removeItem(SIGN_OUT_NOTICE_KEY);
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it.each(['focus', 'online', 'visibilitychange', 'retry'])(
  'checks the current account after a queued %s without overlapping reads',
  async (event) => {
    const { current, held } = await fixture();
    const fresh = deferred();
    current.mockReturnValueOnce(fresh.promise);
    await hint(event);
    expect(current).toHaveBeenCalledTimes(2);
    await act(async () => held.resolve(owner('owner-a')));
    expect(current).toHaveBeenCalledTimes(3);
    await act(async () => fresh.resolve(owner('owner-b')));
    expect(screen.getByText('owner-b')).toBeInTheDocument();
    await advance(60_000);
    expect(current).toHaveBeenCalledTimes(3);
  },
);

it('coalesces event bursts once per in-flight read and stops when events stop', async () => {
  const { current, held } = await fixture();
  const fresh = deferred();
  current.mockReturnValueOnce(fresh.promise).mockResolvedValue(owner('owner-c'));
  for (let i = 0; i < 10; i += 1) {
    await hint('focus');
    await hint('online');
    await hint('visibilitychange');
  }
  expect(current).toHaveBeenCalledTimes(2);
  await act(async () => held.resolve(owner('owner-a')));
  expect(current).toHaveBeenCalledTimes(3);
  await hint('focus');
  await hint('online');
  expect(current).toHaveBeenCalledTimes(3);
  await act(async () => fresh.resolve(owner('owner-b')));
  expect(current).toHaveBeenCalledTimes(4);
  expect(screen.getByText('owner-c')).toBeInTheDocument();
  await advance(60_000);
  expect(current).toHaveBeenCalledTimes(4);
});

it('does not add a follow-up after a successful read without a new event', async () => {
  const { current, held } = await fixture();
  await act(async () => held.resolve(owner('owner-b')));
  expect(screen.getByText('owner-b')).toBeInTheDocument();
  await advance(60_000);
  expect(current).toHaveBeenCalledTimes(2);
});

it('lets the existing outage backoff handle queued hints without an immediate retry', async () => {
  const { current, held } = await fixture();
  current.mockResolvedValue(owner('owner-b'));
  await hint('focus');
  await hint('online');
  await hint('retry');
  await act(async () =>
    held.resolve({ status: 'error', reason: 'identity_unavailable', message: 'Temporary outage' }),
  );
  expect(current).toHaveBeenCalledTimes(2);
  await advance(2999);
  expect(current).toHaveBeenCalledTimes(2);
  await advance(1);
  expect(current).toHaveBeenCalledTimes(3);
  expect(screen.getByText('owner-b')).toBeInTheDocument();
  await advance(60_000);
  expect(current).toHaveBeenCalledTimes(3);
});

it.each(['unclassified', 'thrown'])(
  'does not add automatic retries after a %s failure',
  async (failure) => {
    const { current, held } = await fixture();
    await hint('focus');
    await act(async () => {
      if (failure === 'thrown') held.reject(new Error('Synthetic provider failure'));
      else held.resolve({ status: 'error', message: 'Unclassified refusal' });
    });
    await advance(60_000);
    expect(current).toHaveBeenCalledTimes(2);
    expect(screen.getByText('error')).toBeInTheDocument();
  },
);

it.each(['sign-out', 'sign-in', 'unmount', 'provider'])(
  'discards the queued read when its scope changes through %s',
  async (scope) => {
    const { current, held, view } = await fixture();
    await hint('focus');
    const replacement = vi.fn<AuthProvider['current']>().mockResolvedValue(owner('owner-b'));
    await act(async () => {
      if (scope === 'sign-out') fireEvent.click(screen.getByRole('button', { name: 'Sign out' }));
      else if (scope === 'sign-in') {
        current.mockResolvedValue(owner('owner-b'));
        fireEvent.click(screen.getByRole('button', { name: 'Sign in' }));
      } else if (scope === 'unmount') view.unmount();
      else
        view.rerender(
          <AuthContextProvider
            provider={{ current: replacement, signIn: vi.fn(), signOut: vi.fn() }}
          >
            <View />
          </AuthContextProvider>,
        );
    });
    await act(async () => held.resolve(owner('owner-a')));
    await advance(60_000);
    expect(current).toHaveBeenCalledTimes(scope === 'sign-in' ? 3 : 2);
    if (scope === 'sign-out') expect(screen.getByText('anonymous')).toBeInTheDocument();
    if (scope === 'sign-in' || scope === 'provider')
      expect(screen.getByText('owner-b')).toBeInTheDocument();
    if (scope === 'provider') expect(replacement).toHaveBeenCalledOnce();
  },
);

it('drops pending lifecycle refreshes when silent sign-in starts leaving', async () => {
  const held = deferred();
  const current = vi.fn<AuthProvider['current']>().mockReturnValue(held.promise);
  const silent = vi.fn(() => true);
  mount({ current, signIn: vi.fn(), signOut: vi.fn(), signInSilently: silent });
  await hint('focus');
  await hint('online');
  await act(async () => held.resolve({ status: 'anonymous' }));
  expect(silent).toHaveBeenCalledOnce();
  await hint('focus');
  await advance(10_000);
  expect(current).toHaveBeenCalledOnce();
  expect(screen.getByText('anonymous')).toBeInTheDocument();
});

it('does not let a queued navigation restart silent sign-in after explicit sign-out', async () => {
  const held = deferred();
  const current = vi
    .fn<AuthProvider['current']>()
    .mockResolvedValueOnce({ status: 'anonymous' })
    .mockReturnValueOnce(held.promise)
    .mockResolvedValue({ status: 'anonymous' });
  const silent = vi.fn(() => false);
  mount({ current, signIn: vi.fn(), signOut: vi.fn(), signInSilently: silent });
  await act(async () => {});
  await hint('focus');
  await hint('cma:navigate');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Sign out' })));
  silent.mockReturnValue(true);
  await act(async () => held.resolve({ status: 'anonymous' }));
  await advance(60_000);
  expect(current).toHaveBeenCalledTimes(2);
  expect(silent).toHaveBeenCalledOnce();
  expect(screen.getByText('anonymous')).toBeInTheDocument();
});

it('discards an in-flight identity reply when another tab signs out', async () => {
  const { current, held } = await fixture();
  expect(screen.getByText('owner-a')).toBeInTheDocument();

  window.localStorage.setItem(SIGN_OUT_NOTICE_KEY, crypto.randomUUID());
  fireEvent(window, new StorageEvent('storage', { key: SIGN_OUT_NOTICE_KEY }));
  expect(screen.getByText('anonymous')).toBeInTheDocument();
  expect(screen.getByLabelText('sign-out notice')).toHaveTextContent('received');
  expect(screen.getByLabelText('session reads')).toHaveTextContent('paused');

  await act(async () => held.resolve(owner('owner-a')));
  await hint('focus');
  expect(screen.queryByText('owner-a')).not.toBeInTheDocument();
  expect(current).toHaveBeenCalledTimes(2);
});

it('hides the saved page until its restored session has been verified', async () => {
  const current = vi.fn<AuthProvider['current']>().mockResolvedValueOnce(owner('owner-a'));
  const restored = deferred();
  current.mockReturnValueOnce(restored.promise);
  mount({ current, signIn: vi.fn(), signOut: vi.fn() });
  await act(async () => {});
  expect(screen.getByText('owner-a')).toBeInTheDocument();
  expect(screen.getByLabelText('session reads')).toHaveTextContent('allowed');

  fireEvent(window, new PageTransitionEvent('pagehide', { persisted: true }));
  expect(screen.getByText('checking')).toBeInTheDocument();
  expect(screen.getByLabelText('session reads')).toHaveTextContent('paused');
  fireEvent(window, new PageTransitionEvent('pageshow', { persisted: true }));
  expect(current).toHaveBeenCalledTimes(2);
  expect(screen.queryByText('owner-a')).not.toBeInTheDocument();

  await act(async () => restored.resolve({ status: 'anonymous' }));
  expect(screen.getByText('anonymous')).toBeInTheDocument();
  expect(screen.getByLabelText('session reads')).toHaveTextContent('paused');
});

it('uses the live sign-out notice when storage has no record and ignores malformed data', async () => {
  const channel = new EventTarget();
  const close = vi.fn();
  vi.stubGlobal(
    'BroadcastChannel',
    class {
      addEventListener = channel.addEventListener.bind(channel);
      close = close;
    },
  );
  const { current, view, held } = await fixture();
  await act(async () => held.resolve(owner('owner-a')));

  act(() => {
    channel.dispatchEvent(new MessageEvent('message', { data: { type: 'sign-out' } }));
  });
  expect(screen.getByText('owner-a')).toBeInTheDocument();
  expect(screen.getByLabelText('sign-out notice')).toHaveTextContent('none');

  act(() => {
    channel.dispatchEvent(new MessageEvent('message', { data: crypto.randomUUID() }));
  });
  expect(screen.getByText('anonymous')).toBeInTheDocument();
  expect(screen.getByLabelText('sign-out notice')).toHaveTextContent('received');
  expect(window.localStorage.getItem(SIGN_OUT_NOTICE_KEY)).toBeNull();
  expect(current).toHaveBeenCalledTimes(2);
  view.unmount();
  expect(close).toHaveBeenCalledOnce();
});
