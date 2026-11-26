import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { AuthContextProvider } from '../auth/context';
import type { AuthProvider } from '../auth/types';
import { PUBLIC_RECORD, PUBLIC_TRUST, PUBLIC_USER, publicClient } from '../test/public';
import { ReviewCard } from './ReviewCard';
import { publicCopy } from '../vocabulary/needs';
import '../test/connectedAccounts';

type Outcome = 'lost' | 'setup_required' | 'accepted';
const unexpected: string[] = [];

function fixture(outcomes: Outcome[]) {
  let subject = PUBLIC_USER.id;
  let contextSubject = PUBLIC_USER.id;
  const sent: { id: string; body: unknown; rawBody: string; key: string | null }[] = [];
  const completed = vi.fn();
  const popup = { opener: {}, location: { href: '' }, close: vi.fn() };
  vi.spyOn(window, 'open').mockReturnValue(popup as unknown as Window);
  const json = (body: unknown, status = 200) =>
    new Response(JSON.stringify(body), {
      status,
      headers: { 'Content-Type': 'application/json' },
    });
  const transport = vi.fn<typeof fetch>(async (input, init) => {
    const url = new URL(input instanceof Request ? input.url : String(input), location.origin);
    const method = init?.method ?? (input instanceof Request ? input.method : 'GET');
    if (url.origin === location.origin && url.pathname === '/auth/session' && method === 'GET')
      return json({
        authenticated: true,
        subject,
        visitor: true,
        capabilities: ['launch_triggers'],
      });
    if (
      url.origin === location.origin &&
      url.pathname === `/api/v1/launches/${PUBLIC_TRUST.id}/start` &&
      method === 'POST'
    ) {
      const request = new Request(url, init);
      const rawBody = await request.text();
      sent.push({
        id: url.pathname,
        body: JSON.parse(rawBody),
        rawBody,
        key: request.headers.get('Idempotency-Key'),
      });
      const outcome = outcomes[sent.length - 1];
      if (outcome === 'lost') throw new TypeError('Synthetic response loss');
      if (outcome === 'setup_required')
        return json({ title: 'Sign in again.', code: 'sandbox_credential_setup_required' }, 409);
      if (outcome === 'accepted') return json(PUBLIC_RECORD, 202);
    }
    unexpected.push(`${method} ${url.pathname}`);
    throw new Error('Unexpected request in synthetic credential recovery test');
  });
  vi.stubGlobal('fetch', transport);
  const sdk = createClient({ fetch: transport, requestIdProvider: () => 'storage-boundary' });
  const client = publicClient();
  vi.mocked(client.requests.start).mockImplementation((id, body, options) =>
    sdk.launches.start(id, body, options),
  );
  const provider: AuthProvider = {
    current: async () => ({
      status: 'authenticated',
      user: { ...PUBLIC_USER, id: contextSubject },
    }),
    signIn: vi.fn(),
    signOut: vi.fn(),
  };
  const mount = () =>
    render(
      <AuthContextProvider provider={provider}>
        <ReviewCard
          client={client}
          record={PUBLIC_TRUST}
          expectedSubject={contextSubject}
          onChanged={completed}
          onClose={vi.fn()}
        />
      </AuthContextProvider>,
    );
  return {
    mount,
    sent,
    completed,
    client,
    popup,
    setSubject: (value: string) => {
      subject = value;
    },
    setContextSubject: (value: string) => {
      contextSubject = value;
    },
  };
}

async function clickStart(actor: ReturnType<typeof userEvent.setup>) {
  const button = await screen.findByRole('button', { name: /^(Continue|Try again)$/ });
  await waitFor(() => expect(button).toBeEnabled());
  await actor.click(button);
}

async function checkAccount(actor: ReturnType<typeof userEvent.setup>) {
  await actor.click(await screen.findByRole('button', { name: 'Sign in to CMA' }));
  await actor.click(await screen.findByRole('button', { name: 'Check signed-in account' }));
}

afterEach(() => {
  expect(unexpected.splice(0)).toEqual([]);
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

const storageKey = `cma:draft-credential-recovery:${PUBLIC_TRUST.id}`;

function storageFaults() {
  const backing = sessionStorage;
  const fault = { read: false, write: false, remove: false };
  const failedWrites: string[] = [];
  vi.stubGlobal('sessionStorage', {
    getItem(key: string) {
      if (key === storageKey && fault.read)
        throw new DOMException('Synthetic storage denial', 'SecurityError');
      return backing.getItem(key);
    },
    setItem(key: string, value: string) {
      if (key === storageKey && fault.write) {
        failedWrites.push(value);
        throw new DOMException('Synthetic storage denial', 'SecurityError');
      }
      backing.setItem(key, value);
    },
    removeItem(key: string) {
      if (key === storageKey && fault.remove)
        throw new DOMException('Synthetic storage denial', 'SecurityError');
      backing.removeItem(key);
    },
    clear: () => backing.clear(),
    key: (index: number) => backing.key(index),
    get length() {
      return backing.length;
    },
  } satisfies Storage);
  return { fault, failedWrites, raw: () => backing.getItem(storageKey) };
}

async function startRecovery(
  test: ReturnType<typeof fixture>,
  actor: ReturnType<typeof userEvent.setup>,
) {
  const view = test.mount();
  await actor.click(screen.getByRole('checkbox'));
  await clickStart(actor);
  await checkAccount(actor);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Continue' })).toBeEnabled());
  expect(test.sent).toHaveLength(1);
  return view;
}

it.each([false, true])(
  'control: a failed pre-dispatch write blocks Start and preserves its packet until storage recovers (already unresolved: %s)',
  async (pending) => {
    const test = fixture(['setup_required', ...(pending ? ['lost' as const] : []), 'accepted']);
    const actor = userEvent.setup();
    await startRecovery(test, actor);
    if (pending) {
      await clickStart(actor);
      await screen.findByRole('button', { name: 'Try again' });
    }
    const storage = storageFaults();
    const retained = storage.raw();
    const before = test.sent.length;
    storage.fault.write = true;
    await clickStart(actor);
    await screen.findByText(publicCopy.storageUnavailable);
    expect(test.sent).toHaveLength(before);
    expect(test.completed).not.toHaveBeenCalled();
    expect(storage.raw()).toBe(retained);
    const attempted = JSON.parse(storage.failedWrites.at(-1)!).command;
    expect(attempted.key).toBeTruthy();
    if (pending) expect(attempted.key).toBe(test.sent.at(-1)?.key);
    storage.fault.write = false;
    await clickStart(actor);
    await waitFor(() => expect(test.completed).toHaveBeenCalledOnce());
    expect(test.sent).toHaveLength(before + 1);
    expect(test.sent.at(-1)?.key).toBe(attempted.key);
    expect(test.sent.at(-1)?.body).toEqual(attempted.body);
    expect(storage.raw()).toBeNull();
  },
);

it('control: cleanup failure cannot hide an accepted Start result', async () => {
  const test = fixture(['setup_required', 'accepted']);
  const actor = userEvent.setup();
  await startRecovery(test, actor);
  const storage = storageFaults();
  storage.fault.remove = true;
  await clickStart(actor);
  await waitFor(() => expect(test.completed).toHaveBeenCalledWith(PUBLIC_RECORD));
  expect(test.sent).toHaveLength(2);
  expect(JSON.parse(storage.raw()!).command.key).toBe(test.sent[1]?.key);
  expect(screen.queryByText(publicCopy.storageUnavailable)).not.toBeInTheDocument();
});

it('refuses a fresh Start when an existing unresolved record cannot be read after remount', async () => {
  const test = fixture(['setup_required', 'lost', 'accepted']);
  const actor = userEvent.setup();
  const view = await startRecovery(test, actor);
  await clickStart(actor);
  await screen.findByRole('button', { name: 'Try again' });
  const storage = storageFaults();
  const original = JSON.parse(storage.raw()!);
  expect(original.command.key).toBe(test.sent[1]?.key);
  view.unmount();
  storage.fault.read = true;
  storage.fault.write = true;
  storage.fault.remove = true;
  test.mount();
  expect.soft(screen.queryByRole('alert')).not.toBeNull();
  const checkbox = screen.queryByRole('checkbox');
  if (checkbox !== null && !checkbox.hasAttribute('disabled')) await actor.click(checkbox);
  const submit = screen.queryByRole('button', { name: /^(Continue|Try again)$/ });
  if (submit !== null) await actor.click(submit);
  await act(async () => undefined);
  expect.soft(test.sent).toHaveLength(2);
  expect.soft(test.completed).not.toHaveBeenCalled();
  expect(JSON.parse(storage.raw()!).command).toEqual(original.command);
  if (test.sent.length > 2) {
    expect(test.sent[2]?.id).toBe(test.sent[1]?.id);
    expect(test.sent[2]?.body).toEqual(test.sent[1]?.body);
    expect(test.sent[2]?.key).toBe(test.sent[1]?.key);
  }
});

it('control: a read outage without a new submission can recover the original packet on a later remount', async () => {
  const test = fixture(['setup_required', 'lost', 'accepted']);
  const actor = userEvent.setup();
  const first = await startRecovery(test, actor);
  await clickStart(actor);
  await screen.findByRole('button', { name: 'Try again' });
  const storage = storageFaults();
  const original = storage.raw();
  first.unmount();
  storage.fault.read = true;
  const unavailable = test.mount();
  await act(async () => undefined);
  expect(test.sent).toHaveLength(2);
  expect(storage.raw()).toBe(original);
  unavailable.unmount();
  storage.fault.read = false;
  test.mount();
  await checkAccount(actor);
  expect(test.sent).toHaveLength(2);
  await clickStart(actor);
  await waitFor(() => expect(test.completed).toHaveBeenCalledOnce());
  expect(test.sent[2]).toEqual(test.sent[1]);
  expect(storage.raw()).toBeNull();
});

it('re-reads the original packet explicitly without submitting or accepting a different owner', async () => {
  const test = fixture(['accepted']);
  const actor = userEvent.setup();
  const original = {
    owner: PUBLIC_USER.id,
    confirmedDigest: 'approved-one',
    command: {
      key: 'original-start-key',
      body: { acknowledgements: ['reviewed'], trust_confirmed_digest: 'approved-one' },
    },
  };
  sessionStorage.setItem(storageKey, JSON.stringify(original));
  const storage = storageFaults();
  storage.fault.read = true;
  test.setSubject('different-owner');
  test.setContextSubject('different-owner');
  test.mount();
  await screen.findByText(publicCopy.storageReadUnavailable);
  expect(screen.getByRole('checkbox')).toBeDisabled();
  await clickStart(actor);
  expect(test.sent).toHaveLength(0);
  expect(storage.raw()).toBe(JSON.stringify(original));
  storage.fault.read = false;
  await clickStart(actor);
  await screen.findByRole('region', { name: 'CMA sign-in required' });
  expect(test.sent).toHaveLength(0);
  expect(screen.getByRole('checkbox')).toBeChecked();
  expect(screen.getByRole('checkbox')).toBeDisabled();
  await checkAccount(actor);
  await screen.findByText('Sign in with the same CMA account that made this request.');
  expect(test.sent).toHaveLength(0);
  expect(storage.raw()).toBe(JSON.stringify(original));
  test.setSubject(PUBLIC_USER.id);
  await actor.click(screen.getByRole('button', { name: 'Check signed-in account' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Try again' })).toBeEnabled());
  expect(test.sent).toHaveLength(0);
  await clickStart(actor);
  await waitFor(() => expect(test.completed).toHaveBeenCalledOnce());
  expect(test.sent[0]?.key).toBe(original.command.key);
  expect(test.sent[0]?.rawBody).toBe(JSON.stringify(original.command.body));
  expect(storage.raw()).toBeNull();
});

it.each(['{', 'null', '{"owner":""}'])(
  'keeps unreadable saved data %s intact and blocks submission',
  async (raw) => {
    const test = fixture([]);
    const actor = userEvent.setup();
    sessionStorage.setItem(storageKey, raw);
    test.mount();
    await screen.findByText(publicCopy.storageReadUnavailable);
    expect(screen.getByRole('checkbox')).toBeDisabled();
    await clickStart(actor);
    expect(test.sent).toHaveLength(0);
    expect(test.completed).not.toHaveBeenCalled();
    expect(sessionStorage.getItem(storageKey)).toBe(raw);
    expect(test.client.requests.createOrResumeBound).not.toHaveBeenCalled();
  },
);
