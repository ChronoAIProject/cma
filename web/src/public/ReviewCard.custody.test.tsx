import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, type Launch } from '@cma/client';
import { AuthContextProvider } from '../auth/context';
import { PUBLIC_RECORD, PUBLIC_TRUST, PUBLIC_USER, publicClient } from '../test/public';
import { ReviewCard } from './ReviewCard';
import { readDraftRecovery } from './draftRecovery';
import { publicCopy } from '../vocabulary/needs';
import '../test/connectedAccounts';

const storageKey = `cma:draft-credential-recovery:${PUBLIC_TRUST.id}`;
const failed = new ApiError(503, 'Synthetic receipt failure', 'store_failed');

function fixture() {
  let resolve!: (value: Launch) => void;
  let reject!: (cause: unknown) => void;
  const pending = new Promise<Launch>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  const client = publicClient();
  vi.mocked(client.requests.start).mockReturnValueOnce(pending).mockRejectedValue(failed);
  vi.spyOn(window, 'open').mockReturnValue({
    opener: null,
    location: { href: '' },
    close: vi.fn(),
  } as unknown as Window);
  vi.stubGlobal(
    'fetch',
    vi.fn(
      async () =>
        new Response(
          JSON.stringify({
            authenticated: true,
            subject: PUBLIC_USER.id,
            visitor: true,
            capabilities: ['launch_triggers'],
          }),
          { headers: { 'Content-Type': 'application/json' } },
        ),
    ),
  );
  const completed = vi.fn();
  const mount = (owner: string | undefined = PUBLIC_USER.id) =>
    render(
      <AuthContextProvider
        provider={{
          current: async () => ({ status: 'authenticated', user: PUBLIC_USER }),
          signIn: vi.fn(),
          signOut: vi.fn(),
        }}
      >
        <ReviewCard
          client={client}
          record={PUBLIC_TRUST}
          expectedSubject={owner}
          onChanged={completed}
          onClose={vi.fn()}
        />
      </AuthContextProvider>,
    );
  return { client, mount, resolve, reject, completed };
}

async function submit(actor: ReturnType<typeof userEvent.setup>) {
  const button = await screen.findByRole('button', { name: /^(Continue|Try again)$/ });
  await waitFor(() => expect(button).toBeEnabled());
  await actor.click(button);
}

async function checkAccount(actor: ReturnType<typeof userEvent.setup>) {
  await actor.click(await screen.findByRole('button', { name: 'Sign in to CMA' }));
  await actor.click(await screen.findByRole('button', { name: 'Check signed-in account' }));
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

it('saves the first exact command before dispatch and recovers it after a pending remount', async () => {
  const test = fixture();
  const actor = userEvent.setup();
  const first = test.mount();
  await actor.click(screen.getByRole('checkbox'));
  await submit(actor);
  expect(test.client.requests.start).toHaveBeenCalledOnce();
  const call = vi.mocked(test.client.requests.start).mock.calls[0]!;
  expect(readDraftRecovery(PUBLIC_TRUST.id)).toEqual({
    owner: PUBLIC_USER.id,
    confirmedDigest: 'approved-one',
    command: { key: call[2].idempotencyKey, body: call[1] },
  });
  first.unmount();
  test.mount();
  await checkAccount(actor);
  expect(test.client.requests.start).toHaveBeenCalledOnce();
  await submit(actor);
  expect(vi.mocked(test.client.requests.start).mock.calls[1]).toEqual(call);
  const retained = sessionStorage.getItem(storageKey);
  await act(async () => test.reject(failed));
  expect(sessionStorage.getItem(storageKey)).toBe(retained);
});

it('blocks the first wire dispatch when storage refuses the command, then retries the same packet', async () => {
  const test = fixture();
  const actor = userEvent.setup();
  const backing = sessionStorage;
  const attempts: string[] = [];
  vi.stubGlobal('sessionStorage', {
    getItem: (key: string) => backing.getItem(key),
    setItem: (key: string, value: string) => {
      if (key !== storageKey) return backing.setItem(key, value);
      attempts.push(value);
      throw new Error('Synthetic storage denial');
    },
  });
  test.mount();
  await actor.click(screen.getByRole('checkbox'));
  await submit(actor);
  expect(test.client.requests.start).not.toHaveBeenCalled();
  await screen.findByText(publicCopy.storageUnavailable);
  const prepared = JSON.parse(attempts[0]!).command;
  vi.stubGlobal('sessionStorage', backing);
  await submit(actor);
  expect(test.client.requests.start).toHaveBeenCalledWith(PUBLIC_TRUST.id, prepared.body, {
    idempotencyKey: prepared.key,
  });
  await act(async () => test.resolve(PUBLIC_RECORD));
});

it.each(['accepted', 'failed', 'credential_refusal'] as const)(
  'a late %s cannot change a replacement owner or command',
  async (outcome) => {
    const test = fixture();
    const actor = userEvent.setup();
    const view = test.mount();
    await actor.click(screen.getByRole('checkbox'));
    await submit(actor);
    view.unmount();
    const replacement = JSON.stringify({
      owner: 'another-owner',
      command: { key: 'replacement-key', body: { acknowledgements: ['replacement'] } },
    });
    sessionStorage.setItem(storageKey, replacement);
    await act(async () => {
      if (outcome === 'accepted') test.resolve(PUBLIC_RECORD);
      else
        test.reject(
          outcome === 'failed'
            ? failed
            : new ApiError(409, 'Sign in', 'sandbox_credential_setup_required'),
        );
    });
    expect(sessionStorage.getItem(storageKey)).toBe(replacement);
  },
);

it('a late failure cannot resurrect a command after its recovery was removed', async () => {
  const test = fixture();
  const actor = userEvent.setup();
  const view = test.mount();
  await actor.click(screen.getByRole('checkbox'));
  await submit(actor);
  view.unmount();
  sessionStorage.removeItem(storageKey);
  await act(async () => test.reject(failed));
  expect(sessionStorage.getItem(storageKey)).toBeNull();
});

it.each(['accepted-first', 'error-first'] as const)(
  'keeps same-key acceptance after remount when responses arrive %s',
  async (order) => {
    const test = fixture();
    const actor = userEvent.setup();
    const initial = test.mount();
    await actor.click(screen.getByRole('checkbox'));
    await submit(actor);
    await act(async () => test.reject(failed));
    initial.unmount();

    let rejectOlder!: (cause: unknown) => void;
    let acceptNewer!: (value: Launch) => void;
    vi.mocked(test.client.requests.start)
      .mockReturnValueOnce(new Promise((_, reject) => (rejectOlder = reject)))
      .mockReturnValueOnce(new Promise((resolve) => (acceptNewer = resolve)));
    const older = test.mount();
    await checkAccount(actor);
    await submit(actor);
    older.unmount();
    test.mount();
    await checkAccount(actor);
    expect(test.client.requests.start).toHaveBeenCalledTimes(2);
    await submit(actor);
    const calls = vi.mocked(test.client.requests.start).mock.calls;
    expect(calls).toHaveLength(3);
    expect(calls[1]).toEqual(calls[0]);
    expect(calls[2]).toEqual(calls[0]);
    const retained = sessionStorage.getItem(storageKey);
    expect(retained).not.toBeNull();

    if (order === 'error-first') {
      await act(async () => rejectOlder(failed));
      expect(sessionStorage.getItem(storageKey)).toBe(retained);
      expect(test.completed).not.toHaveBeenCalled();
    }
    await act(async () => acceptNewer(PUBLIC_RECORD));
    expect(test.completed).toHaveBeenCalledExactlyOnceWith(PUBLIC_RECORD);
    expect(sessionStorage.getItem(storageKey)).toBeNull();
    if (order === 'accepted-first') await act(async () => rejectOlder(failed));
    expect(sessionStorage.getItem(storageKey)).toBeNull();
    expect(test.completed).toHaveBeenCalledExactlyOnceWith(PUBLIC_RECORD);
    expect(test.client.requests.start).toHaveBeenCalledTimes(3);
  },
);

it('refuses dispatch when a replacement arrives during the owner preflight', async () => {
  const test = fixture();
  const original = {
    owner: PUBLIC_USER.id,
    command: { key: 'retained-command', body: { acknowledgements: [] } },
  };
  sessionStorage.setItem(storageKey, JSON.stringify(original));
  const actor = userEvent.setup();
  test.mount();
  await checkAccount(actor);
  let checked!: (value: Response) => void;
  vi.mocked(fetch).mockReturnValueOnce(
    new Promise((resolve) => {
      checked = resolve;
    }),
  );
  await submit(actor);
  expect(test.client.requests.start).not.toHaveBeenCalled();
  const replacement = JSON.stringify({
    owner: 'new-owner',
    command: { key: 'new-command', body: {} },
  });
  sessionStorage.setItem(storageKey, replacement);
  await act(async () =>
    checked(
      new Response(
        JSON.stringify({
          authenticated: true,
          subject: PUBLIC_USER.id,
          visitor: true,
          capabilities: ['launch_triggers'],
        }),
        { headers: { 'Content-Type': 'application/json' } },
      ),
    ),
  );
  await screen.findByText(publicCopy.storageChanged);
  expect(test.client.requests.start).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(storageKey)).toBe(replacement);
  await submit(actor);
  await screen.findByRole('region', { name: 'CMA sign-in required' });
  expect(test.client.requests.start).not.toHaveBeenCalled();
  await checkAccount(actor);
  await screen.findByText('Sign in with the same CMA account that made this request.');
  expect(test.client.requests.start).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(storageKey)).toBe(replacement);
});

it('does not dispatch an initial Start without a known owner', async () => {
  const test = fixture();
  const actor = userEvent.setup();
  render(
    <ReviewCard client={test.client} record={PUBLIC_TRUST} onChanged={vi.fn()} onClose={vi.fn()} />,
  );
  await actor.click(screen.getByRole('checkbox'));
  await submit(actor);
  await screen.findByText('Your CMA account could not be checked. Try again.');
  expect(test.client.requests.start).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(storageKey)).toBeNull();
});
