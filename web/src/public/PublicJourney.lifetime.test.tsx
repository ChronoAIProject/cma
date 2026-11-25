import { StrictMode } from 'react';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, type Launch } from '@cma/client';
import { AuthContextProvider, useAuth } from '../auth/context';
import { PUBLIC_RECORD, PUBLIC_USER, publicClient } from '../test/public';
import { PublicJourney } from './PublicJourney';
import { createIntent, readIntent, writeIntent } from './intent';

const route = { kind: 'public_trigger', publicId: 'lifetime-test' } as const;
const path = '/t/lifetime-test';
const key = 'cma:public-intent';

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

function session(subject = PUBLIC_USER.id) {
  return new Response(
    JSON.stringify({
      authenticated: true,
      subject,
      visitor: true,
      capabilities: PUBLIC_USER.capabilities,
    }),
    { headers: { 'Content-Type': 'application/json' } },
  );
}

function setup(client = publicClient(), remountOwner = true) {
  let owner = PUBLIC_USER.id;
  const provider = {
    current: vi.fn(async () => ({
      status: 'authenticated' as const,
      user: { ...PUBLIC_USER, id: owner },
    })),
    signIn: vi.fn(async () => undefined),
    signOut: vi.fn(async () => undefined),
  };
  const navigate = vi.fn();
  function Journey({ shown }: { shown: boolean }) {
    const { state } = useAuth();
    const subject = state.status === 'authenticated' ? state.user.id : undefined;
    return (
      <>
        <output aria-label="Current account">{subject}</output>
        {shown ? (
          <PublicJourney
            key={remountOwner ? subject : undefined}
            client={client}
            route={route}
            onNavigate={navigate}
          />
        ) : (
          <p>Another route</p>
        )}
      </>
    );
  }
  const tree = (shown = true) => (
    <StrictMode>
      <AuthContextProvider provider={provider}>
        <Journey shown={shown} />
      </AuthContextProvider>
    </StrictMode>
  );
  const view = render(tree());
  return {
    ...view,
    client,
    navigate,
    user: userEvent.setup(),
    show: (shown: boolean) => view.rerender(tree(shown)),
    changeOwner: async (next: string) => {
      owner = next;
      await act(async () => window.dispatchEvent(new Event('focus')));
      await waitFor(() => expect(screen.getByLabelText('Current account')).toHaveTextContent(next));
    },
  };
}

afterEach(() => {
  sessionStorage.clear();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

it('releases obsolete busy state when identity changes without a parent remount', async () => {
  const response = deferred<Launch>();
  const client = publicClient();
  vi.mocked(client.requests.createOrResumeBound).mockReturnValueOnce(response.promise);
  const test = setup(client, false);
  await test.user.click(await screen.findByRole('button', { name: 'Open in CMA' }));
  await test.changeOwner('another-owner');
  expect(screen.getByRole('button', { name: 'Open in CMA' })).toBeEnabled();
  await test.changeOwner(PUBLIC_USER.id);
  await test.user.click(screen.getByRole('button', { name: 'Open in CMA' }));
  await act(async () => response.resolve(PUBLIC_RECORD));
  await waitFor(() => expect(test.navigate).toHaveBeenCalledOnce());
  expect(client.requests.createOrResumeBound).toHaveBeenCalledOnce();
});

it('shows a current accepted result even when reading storage for cleanup fails', async () => {
  const response = deferred<Launch>();
  const client = publicClient();
  vi.mocked(client.requests.createOrResumeBound).mockReturnValueOnce(response.promise);
  const test = setup(client);
  await test.user.click(await screen.findByRole('button', { name: 'Open in CMA' }));
  const retained = sessionStorage.getItem(key);
  const read = vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
    throw new Error('Storage unavailable');
  });
  await act(async () => response.resolve(PUBLIC_RECORD));
  expect(test.navigate).toHaveBeenCalledOnce();
  read.mockRestore();
  expect(sessionStorage.getItem(key)).toBe(retained);
});

it('reports failed refusal persistence and keeps the original dispatched packet', async () => {
  const response = deferred<Launch>();
  const client = publicClient();
  vi.mocked(client.requests.createOrResumeBound).mockReturnValueOnce(response.promise);
  const test = setup(client);
  await test.user.click(await screen.findByRole('button', { name: 'Open in CMA' }));
  const retained = sessionStorage.getItem(key);
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new Error('Storage unavailable');
  });
  await act(async () =>
    response.reject(new ApiError(409, 'Sign in again.', 'sandbox_credential_setup_required')),
  );
  expect(await screen.findByRole('alert')).toHaveTextContent('could not save');
  expect(sessionStorage.getItem(key)).toBe(retained);
  expect(test.navigate).not.toHaveBeenCalled();
});

async function recovery() {
  const client = publicClient();
  vi.mocked(client.requests.createOrResumeBound).mockRejectedValueOnce(
    new ApiError(409, 'Sign in again.', 'sandbox_credential_setup_required'),
  );
  const fetch = vi.fn().mockResolvedValueOnce(session());
  vi.stubGlobal('fetch', fetch);
  vi.spyOn(window, 'open').mockReturnValue({
    opener: null,
    location: { href: '' },
    close: vi.fn(),
  } as unknown as Window);
  const test = setup(client);
  await test.user.click(await screen.findByRole('button', { name: 'Open in CMA' }));
  await test.user.click(await screen.findByRole('button', { name: 'Sign in to CMA' }));
  await test.user.click(screen.getByRole('button', { name: 'Check signed-in account' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Open in CMA' })).toBeEnabled());
  const check = deferred<Response>();
  // Deliberately ignores AbortSignal to exercise the continuation guard too.
  fetch.mockReturnValueOnce(check.promise);
  await test.user.click(screen.getByRole('button', { name: 'Open in CMA' }));
  expect(fetch).toHaveBeenCalledTimes(2);
  expect(client.requests.createOrResumeBound).toHaveBeenCalledOnce();
  return { ...test, check, fetch };
}

it.each(['resolve', 'reject'] as const)(
  'stops an obsolete preflight after route exit: %s',
  async (outcome) => {
    const test = await recovery();
    const retained = sessionStorage.getItem(key);
    test.show(false);
    await act(async () => {
      if (outcome === 'resolve') test.check.resolve(session());
      else test.check.reject(new Error('late preflight failure'));
    });
    expect(test.client.requests.createOrResumeBound).toHaveBeenCalledOnce();
    expect(sessionStorage.getItem(key)).toBe(retained);
    expect(test.navigate).not.toHaveBeenCalled();
    expect(screen.getByText('Another route')).toBeInTheDocument();
  },
);

it('aborts the account check and leaves recovery intact across A to B to A', async () => {
  const test = await recovery();
  const retained = sessionStorage.getItem(key);
  await test.changeOwner('another-owner');
  await test.changeOwner(PUBLIC_USER.id);
  await act(async () => test.check.resolve(session()));
  expect(test.client.requests.createOrResumeBound).toHaveBeenCalledOnce();
  expect(test.fetch.mock.calls[1]?.[1].signal.aborted).toBe(true);
  expect(sessionStorage.getItem(key)).toBe(retained);
  expect(screen.getByRole('button', { name: 'Open in CMA' })).toBeDisabled();
  expect(test.navigate).not.toHaveBeenCalled();
});

it.each(['accepted', 'credential refusal', 'settled refusal', 'unknown'] as const)(
  'blocks B without consuming A when its retired observer returns a late %s',
  async (outcome) => {
    const response = deferred<Launch>();
    const client = publicClient();
    vi.mocked(client.requests.createOrResumeBound)
      .mockReturnValueOnce(response.promise)
      .mockRejectedValueOnce(new ApiError(503, 'Unknown', 'service_unavailable'))
      .mockResolvedValue(PUBLIC_RECORD);
    const test = setup(client);
    await test.user.click(await screen.findByRole('button', { name: 'Open in CMA' }));
    const original = readIntent(path)!;
    await test.changeOwner('another-owner');
    await test.user.click(await screen.findByRole('button', { name: 'Open in CMA' }));
    await screen.findByRole('alert');
    const retained = sessionStorage.getItem(key);
    const current = readIntent(path)!;
    expect(current).toEqual(original);
    expect(screen.getByRole('link', { name: 'Return to saved request' })).toHaveAttribute(
      'href',
      path,
    );
    expect(screen.getByRole('button', { name: 'Open in CMA' })).toBeDisabled();
    await act(async () => {
      if (outcome === 'accepted') response.resolve(PUBLIC_RECORD);
      else
        response.reject(
          outcome === 'credential refusal'
            ? new ApiError(409, 'Sign in again.', 'sandbox_credential_setup_required')
            : outcome === 'settled refusal'
              ? new ApiError(403, 'Denied', 'forbidden')
              : new Error('connection lost'),
        );
    });
    expect(sessionStorage.getItem(key)).toBe(retained);
    expect(test.navigate).not.toHaveBeenCalled();
    expect(client.requests.createOrResumeBound).toHaveBeenCalledOnce();
    await test.changeOwner(PUBLIC_USER.id);
    if (outcome !== 'accepted') {
      await waitFor(() => expect(client.requests.createOrResumeBound).toHaveBeenCalledTimes(2));
      await test.user.click(await screen.findByRole('button', { name: 'Try again' }));
    }
    await waitFor(() => expect(test.navigate).toHaveBeenCalledOnce());
    const calls = vi.mocked(client.requests.createOrResumeBound).mock.calls;
    expect(calls).toHaveLength(outcome === 'accepted' ? 1 : 3);
    for (const call of calls) expect(call).toEqual(calls[0]);
  },
);

it.each(
  (['pending', 'accepted'] as const).flatMap((timing) =>
    [-1, 0, 1, -30 * 60 * 1000].map((offset) => ({ timing, offset })),
  ),
)('recovers the same $timing command at expiry offset $offset', async ({ timing, offset }) => {
  const response = deferred<Launch>();
  const client = publicClient();
  vi.mocked(client.requests.createOrResumeBound).mockReturnValueOnce(response.promise);
  const test = setup(client);
  await test.user.click(await screen.findByRole('button', { name: 'Open in CMA' }));
  const retained = sessionStorage.getItem(key);
  const original = readIntent(path)!;
  test.show(false);
  if (timing === 'accepted') {
    await act(async () => response.resolve(PUBLIC_RECORD));
    expect(sessionStorage.getItem(key)).toBe(retained);
    expect(test.navigate).not.toHaveBeenCalled();
  }
  vi.spyOn(Date, 'now').mockReturnValue(original.expiresAt + offset);
  test.show(true);
  await screen.findByRole('heading', { name: 'Research assistant' });
  await act(async () => response.resolve(PUBLIC_RECORD));
  await waitFor(() => expect(test.navigate).toHaveBeenCalledOnce());
  expect(test.navigate).toHaveBeenCalledWith(
    { kind: 'launch', launchId: PUBLIC_RECORD.id },
    { replace: true },
  );
  expect(client.requests.createOrResumeBound).toHaveBeenCalledOnce();
  expect(readIntent(path)).toBeUndefined();
});

it('keeps an expired submitted packet without dispatch or re-key when its result is absent', async () => {
  const original = { ...createIntent(route), owner: PUBLIC_USER.id, submitted: true };
  writeIntent(original);
  const retained = sessionStorage.getItem(key);
  vi.spyOn(Date, 'now').mockReturnValue(original.expiresAt);
  const test = setup();
  expect(await screen.findByRole('alert')).toHaveTextContent('Your original request is saved');
  await test.user.click(screen.getByRole('button', { name: 'Open in CMA' }));
  const retry = screen.queryByRole('button', { name: 'Try again' });
  if (retry !== null) await test.user.click(retry);
  expect(test.client.requests.createOrResumeBound).not.toHaveBeenCalled();
  expect(test.navigate).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(key)).toBe(retained);
  expect(readIntent(path)).toEqual(original);
});
