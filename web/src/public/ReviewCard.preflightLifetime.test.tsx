import { StrictMode, useLayoutEffect } from 'react';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { type Launch } from '@cma/client';
import { AuthContextProvider } from '../auth/context';
import * as credentialSetup from '../auth/credentialSetup';
import { Presence } from '../ui/Presence';
import { PUBLIC_RECORD, PUBLIC_TRUST, PUBLIC_USER, publicClient } from '../test/public';
import { readDraftRecovery } from './draftRecovery';
import { ReviewCard } from './ReviewCard';
import '../test/connectedAccounts';

const storageKey = `cma:draft-credential-recovery:${PUBLIC_TRUST.id}`;
const recovery = {
  owner: PUBLIC_USER.id,
  confirmedDigest: 'approved-one',
  command: {
    key: 'retained-start',
    body: { acknowledgements: ['reviewed'], trust_confirmed_digest: 'approved-one' },
  },
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

function fixture(strict = false) {
  sessionStorage.setItem(storageKey, JSON.stringify(recovery));
  const client = publicClient();
  const completed = vi.fn();
  const preflight = vi.spyOn(credentialSetup, 'checkCredentialRecoveryOwner').mockResolvedValue();
  const storage = vi.spyOn(Storage.prototype, 'setItem');
  const writes = () => storage.mock.calls.filter(([key]) => key === storageKey).length;
  vi.spyOn(window, 'open').mockReturnValue({
    opener: null,
    location: { href: '' },
    close: vi.fn(),
  } as unknown as Window);
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input) => {
      if (input !== '/auth/session') throw new Error('Unexpected external fetch');
      return new Response(
        JSON.stringify({
          authenticated: true,
          subject: PUBLIC_USER.id,
          visitor: true,
          capabilities: ['launch_triggers'],
        }),
        { headers: { 'Content-Type': 'application/json' } },
      );
    }),
  );
  const provider = {
    current: async () => ({ status: 'authenticated' as const, user: PUBLIC_USER }),
    signIn: vi.fn(),
    signOut: vi.fn(),
  };
  const element = (
    owner = PUBLIC_USER.id,
    id = PUBLIC_TRUST.id,
    open = true,
    onCommit?: () => void,
  ) => {
    const content = (
      <AuthContextProvider provider={provider}>
        <Presence>
          {open ? (
            <ReviewCard
              client={client}
              record={{ ...PUBLIC_TRUST, id }}
              expectedSubject={owner}
              onChanged={completed}
              onClose={vi.fn()}
            />
          ) : null}
        </Presence>
        <CommitProbe onCommit={onCommit} />
      </AuthContextProvider>
    );
    return strict ? <StrictMode>{content}</StrictMode> : content;
  };
  return { client, completed, preflight, writes, element };
}

function CommitProbe({ onCommit }: { readonly onCommit?: () => void }) {
  useLayoutEffect(() => onCommit?.(), [onCommit]);
  return null;
}

async function allowRetry(actor: ReturnType<typeof userEvent.setup>) {
  await actor.click(await screen.findByRole('button', { name: 'Sign in to CMA' }));
  await actor.click(await screen.findByRole('button', { name: 'Check signed-in account' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Try again' })).toBeEnabled());
}

afterEach(() => {
  expect(vi.mocked(fetch).mock.calls.every(([input]) => input === '/auth/session')).toBe(true);
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

for (const ending of ['unmount', 'owner change', 'Launch change', 'exit frame'] as const) {
  it.each(['resolve', 'reject'] as const)(
    `stops a preflight that ignores abort and later %ss after ${ending}`,
    async (outcome) => {
      const test = fixture();
      const actor = userEvent.setup();
      const view = render(test.element());
      await allowRetry(actor);
      const held = deferred<void>();
      test.preflight.mockReturnValueOnce(held.promise);
      await actor.click(screen.getByRole('button', { name: 'Try again' }));
      expect(test.preflight).toHaveBeenCalledOnce();
      const signal = test.preflight.mock.calls[0]?.[1];
      const writes = test.writes();
      const retained = sessionStorage.getItem(storageKey);
      if (ending === 'unmount') view.unmount();
      else if (ending === 'owner change') view.rerender(test.element('different-owner'));
      else if (ending === 'Launch change')
        view.rerender(test.element(PUBLIC_USER.id, 'different-launch'));
      else view.rerender(test.element(PUBLIC_USER.id, PUBLIC_TRUST.id, false));
      await act(async () => {
        if (outcome === 'resolve') held.resolve();
        else
          held.reject(
            new credentialSetup.CredentialAccountError('Synthetic delayed account failure'),
          );
      });
      expect.soft(test.client.requests.start).not.toHaveBeenCalled();
      expect.soft(signal?.aborted).toBe(true);
      expect.soft(test.writes()).toBe(writes);
      expect.soft(sessionStorage.getItem(storageKey)).toBe(retained);
      expect(test.completed).not.toHaveBeenCalled();
    },
  );
}

it('invalidates the old preflight during the owner-change commit', async () => {
  const test = fixture();
  const actor = userEvent.setup();
  const view = render(test.element());
  await allowRetry(actor);
  const held = deferred<void>();
  test.preflight.mockReturnValueOnce(held.promise);
  await actor.click(screen.getByRole('button', { name: 'Try again' }));
  const signal = test.preflight.mock.calls[0]?.[1];
  let abortedAtCommit: boolean | undefined;
  view.rerender(
    test.element('different-owner', PUBLIC_TRUST.id, true, () => {
      abortedAtCommit = signal?.aborted;
      held.resolve();
    }),
  );
  await act(async () => {
    await held.promise;
  });
  expect(abortedAtCommit).toBe(true);
  expect(test.client.requests.start).not.toHaveBeenCalled();
});

it('permits the original explicit retry after StrictMode effect replay', async () => {
  const test = fixture(true);
  const actor = userEvent.setup();
  render(test.element());
  await allowRetry(actor);
  expect(test.client.requests.start).not.toHaveBeenCalled();
  await actor.click(screen.getByRole('button', { name: 'Try again' }));
  await waitFor(() => expect(test.completed).toHaveBeenCalledWith(PUBLIC_RECORD));
  expect(test.preflight.mock.calls[0]?.[1]?.aborted).toBe(false);
  expect(test.client.requests.start).toHaveBeenCalledWith(PUBLIC_TRUST.id, recovery.command.body, {
    idempotencyKey: recovery.command.key,
  });
  expect(sessionStorage.getItem(storageKey)).toBeNull();
});

it.each(['accepted', 'lost'] as const)(
  'keeps handling an already dispatched %s result after unmount',
  async (outcome) => {
    const test = fixture();
    const actor = userEvent.setup();
    const held = deferred<Launch>();
    vi.mocked(test.client.requests.start).mockReturnValueOnce(held.promise);
    const view = render(test.element());
    await allowRetry(actor);
    await actor.click(screen.getByRole('button', { name: 'Try again' }));
    expect(test.client.requests.start).toHaveBeenCalledOnce();
    expect(vi.mocked(test.client.requests.start).mock.calls[0]?.[2].signal).toBeUndefined();
    view.unmount();
    await act(async () => {
      if (outcome === 'accepted') held.resolve(PUBLIC_RECORD);
      else held.reject(new TypeError('Synthetic response loss'));
    });
    if (outcome === 'accepted') {
      expect(test.completed).toHaveBeenCalledWith(PUBLIC_RECORD);
      expect(sessionStorage.getItem(storageKey)).toBeNull();
    } else {
      expect(test.completed).not.toHaveBeenCalled();
      expect(readDraftRecovery(PUBLIC_TRUST.id)).toEqual(recovery);
      render(test.element());
      await allowRetry(actor);
      expect(test.client.requests.start).toHaveBeenCalledOnce();
      vi.mocked(test.client.requests.start).mockResolvedValueOnce(PUBLIC_RECORD);
      await actor.click(screen.getByRole('button', { name: 'Try again' }));
      expect(vi.mocked(test.client.requests.start).mock.calls[1]).toEqual(
        vi.mocked(test.client.requests.start).mock.calls[0],
      );
    }
  },
);
