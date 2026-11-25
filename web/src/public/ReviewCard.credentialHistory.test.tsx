import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { AuthContextProvider } from '../auth/context';
import type { AuthProvider } from '../auth/types';
import { PUBLIC_RECORD, PUBLIC_TRUST, PUBLIC_USER, publicClient } from '../test/public';
import { readDraftRecovery } from './draftRecovery';
import { ReviewCard } from './ReviewCard';
import '../test/connectedAccounts';

type Outcome =
  | 'lost'
  | 'setup_required'
  | 'unavailable_setup'
  | 'store_failed'
  | 'idempotency_in_flight'
  | 'idempotency_recovery_required'
  | 'accepted'
  | 401
  | 403
  | 404
  | 503;
const unexpected: string[] = [];

function fixture(outcomes: Outcome[]) {
  let subject = PUBLIC_USER.id;
  let contextSubject = PUBLIC_USER.id;
  const sent: { id: string; body: string; key: string | null }[] = [];
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
      sent.push({
        id: url.pathname,
        body: await request.text(),
        key: request.headers.get('Idempotency-Key'),
      });
      const outcome = outcomes[sent.length - 1];
      if (outcome === 'lost') throw new TypeError('Synthetic response loss');
      if (outcome === 'setup_required')
        return json({ title: 'Sign in again.', code: 'sandbox_credential_setup_required' }, 409);
      if (outcome === 'unavailable_setup')
        return json({ title: 'Unavailable', code: 'sandbox_credential_setup_required' }, 503);
      if (
        outcome === 'store_failed' ||
        outcome === 'idempotency_in_flight' ||
        outcome === 'idempotency_recovery_required'
      )
        return json(
          { title: 'Command receipt unavailable', code: outcome },
          outcome === 'store_failed' ? 503 : 409,
        );
      if (typeof outcome === 'number')
        return json(
          {
            title: 'Synthetic access refusal',
            code:
              outcome === 401
                ? 'not_authenticated'
                : outcome === 403
                  ? 'forbidden'
                  : outcome === 404
                    ? 'launch_not_found'
                    : 'launch_service_unavailable',
          },
          outcome,
        );
      if (outcome === 'accepted') return json(PUBLIC_RECORD, 202);
    }
    unexpected.push(`${method} ${url.pathname}`);
    throw new Error('Unexpected request in synthetic credential recovery test');
  });
  vi.stubGlobal('fetch', transport);
  const sdk = createClient({ fetch: transport, requestIdProvider: () => 'credential-history' });
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

it.each([401, 403, 404, 503, 'unavailable_setup'] as const)(
  'retains an unresolved post-sign-in Start in storage and through remount after %s',
  async (status) => {
    const test = fixture(['setup_required', 'lost', status, 'accepted']);
    const actor = userEvent.setup();
    const view = test.mount();
    await actor.click(screen.getByRole('checkbox'));
    await clickStart(actor);
    await checkAccount(actor);
    expect(test.sent).toHaveLength(1);
    await clickStart(actor);
    await screen.findByRole('button', { name: 'Try again' });
    const unresolved = readDraftRecovery(PUBLIC_TRUST.id)!;
    expect(unresolved.command?.key).toBe(test.sent[1]?.key);
    expect(test.sent[1]?.key).not.toBe(test.sent[0]?.key);
    await clickStart(actor);
    await waitFor(() =>
      expect(vi.mocked(test.client.requests.start).mock.settledResults[2]?.type).toBe('rejected'),
    );
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /^(Continue|Try again)$/ })).toBeEnabled(),
    );
    expect(test.sent[2]).toEqual(test.sent[1]);
    expect.soft(readDraftRecovery(PUBLIC_TRUST.id)?.command).toEqual(unresolved.command);
    view.unmount();
    test.setSubject('different-owner');
    test.setContextSubject('different-owner');
    test.mount();
    await checkAccount(actor);
    await screen.findByText('Sign in with the same CMA account that made this request.');
    expect(test.sent).toHaveLength(3);
    expect(readDraftRecovery(PUBLIC_TRUST.id)?.owner).toBe(PUBLIC_USER.id);
    expect(screen.getByRole('checkbox')).toBeChecked();
    test.setSubject(PUBLIC_USER.id);
    await actor.click(screen.getByRole('button', { name: 'Check signed-in account' }));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /^(Continue|Try again)$/ })).toBeEnabled(),
    );
    expect(test.sent).toHaveLength(3);
    await clickStart(actor);
    await waitFor(() => expect(test.completed).toHaveBeenCalledOnce());
    expect(test.sent[3]?.id).toBe(test.sent[1]?.id);
    expect(test.sent[3]?.body).toEqual(test.sent[1]?.body);
    expect(test.client.requests.createOrResumeBound).not.toHaveBeenCalled();
    expect(test.sent[3]?.key).toBe(test.sent[1]?.key);
  },
);

it('control: a typed final setup refusal after response loss permits a fresh explicit Start', async () => {
  const test = fixture(['lost', 'setup_required', 'accepted']);
  const actor = userEvent.setup();
  const view = test.mount();
  await actor.click(screen.getByRole('checkbox'));
  await clickStart(actor);
  await screen.findByRole('button', { name: 'Try again' });
  await clickStart(actor);
  await screen.findByRole('region', { name: 'CMA sign-in required' });
  expect(test.sent[1]).toEqual(test.sent[0]);
  expect(readDraftRecovery(PUBLIC_TRUST.id)).toEqual({
    owner: PUBLIC_USER.id,
    confirmedDigest: 'approved-one',
  });
  view.unmount();
  test.mount();
  await checkAccount(actor);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Continue' })).toBeEnabled());
  expect(test.sent).toHaveLength(2);
  await clickStart(actor);
  await waitFor(() => expect(test.completed).toHaveBeenCalledOnce());
  expect(test.sent[2]?.body).toEqual(test.sent[0]?.body);
  expect(test.sent[2]?.key).not.toBe(test.sent[0]?.key);
  expect(readDraftRecovery(PUBLIC_TRUST.id)).toBeUndefined();
});

it('control: remount and a different account alone do not discard an unresolved recovered Start', async () => {
  const test = fixture(['setup_required', 'lost', 'accepted']);
  const actor = userEvent.setup();
  const view = test.mount();
  await actor.click(screen.getByRole('checkbox'));
  await clickStart(actor);
  await checkAccount(actor);
  await clickStart(actor);
  await screen.findByRole('button', { name: 'Try again' });
  const retained = readDraftRecovery(PUBLIC_TRUST.id);
  expect(retained?.command?.key).toBe(test.sent[1]?.key);
  view.unmount();
  test.setSubject('different-owner');
  test.setContextSubject('different-owner');
  test.mount();
  expect(screen.getByRole('checkbox')).toBeDisabled();
  await checkAccount(actor);
  await screen.findByText('Sign in with the same CMA account that made this request.');
  expect(readDraftRecovery(PUBLIC_TRUST.id)).toEqual(retained);
  expect(test.sent).toHaveLength(2);
  test.setSubject(PUBLIC_USER.id);
  await actor.click(screen.getByRole('button', { name: 'Check signed-in account' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Try again' })).toBeEnabled());
  expect(test.sent).toHaveLength(2);
  await clickStart(actor);
  await waitFor(() => expect(test.completed).toHaveBeenCalledOnce());
  expect(test.sent[2]).toEqual(test.sent[1]);
  expect(readDraftRecovery(PUBLIC_TRUST.id)).toBeUndefined();
});

it('retains an unwritten Start receipt through pending recovery and a Resolving state read', async () => {
  const test = fixture(['store_failed', 'idempotency_in_flight', 'idempotency_recovery_required']);
  const actor = userEvent.setup();
  test.mount();
  await actor.click(screen.getByRole('checkbox'));
  await clickStart(actor);
  const retained = readDraftRecovery(PUBLIC_TRUST.id)!;
  expect(retained.owner).toBe(PUBLIC_USER.id);
  expect(retained.confirmedDigest).toBe('approved-one');
  expect(retained.command?.key).toBe(test.sent[0]?.key);
  expect(JSON.stringify(retained.command?.body)).toBe(test.sent[0]?.body);
  expect(test.completed).not.toHaveBeenCalled();
  await clickStart(actor);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Try again' })).toBeEnabled());
  expect(test.sent[1]).toEqual(test.sent[0]);
  expect(readDraftRecovery(PUBLIC_TRUST.id)).toEqual(retained);
  expect(test.completed).not.toHaveBeenCalled();
  const advanced = { ...PUBLIC_RECORD, revision: 2, event_head: 3 };
  vi.mocked(test.client.requests.read).mockResolvedValueOnce(advanced);
  await clickStart(actor);
  await waitFor(() => expect(test.completed).toHaveBeenCalledWith(advanced));
  expect(test.client.requests.read).toHaveBeenCalledWith(PUBLIC_TRUST.id);
  expect(test.sent[2]).toEqual(test.sent[0]);
  expect(test.sent).toHaveLength(3);
  expect(readDraftRecovery(PUBLIC_TRUST.id)).toEqual(retained);
  expect(screen.getByRole('checkbox')).toBeChecked();
  expect(screen.getByRole('checkbox')).toBeDisabled();
  expect(test.client.requests.createOrResumeBound).not.toHaveBeenCalled();
});

it('replays the original Start after a receipt acknowledgement error without replacing its key', async () => {
  const test = fixture(['store_failed', 'accepted']);
  const actor = userEvent.setup();
  test.mount();
  await actor.click(screen.getByRole('checkbox'));
  await clickStart(actor);
  expect(readDraftRecovery(PUBLIC_TRUST.id)?.command?.key).toBe(test.sent[0]?.key);
  expect(test.completed).not.toHaveBeenCalled();
  await clickStart(actor);
  await waitFor(() => expect(test.completed).toHaveBeenCalledWith(PUBLIC_RECORD));
  expect(test.sent).toHaveLength(2);
  expect(test.sent[1]).toEqual(test.sent[0]);
  expect(readDraftRecovery(PUBLIC_TRUST.id)).toBeUndefined();
  expect(test.client.requests.createOrResumeBound).not.toHaveBeenCalled();
});

it.each([false, true])(
  'retains a Draft Start through RecoveryRequired and remount until its original receipt arrives (prior uncertainty: %s)',
  async (priorUncertainty) => {
    const prefix: Outcome[] = priorUncertainty ? ['lost', 'idempotency_in_flight'] : [];
    const test = fixture([...prefix, 'idempotency_recovery_required', 'accepted']);
    vi.mocked(test.client.requests.read).mockResolvedValue(PUBLIC_TRUST);
    const actor = userEvent.setup();
    const view = test.mount();
    await actor.click(screen.getByRole('checkbox'));
    await clickStart(actor);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Try again' })).toBeEnabled());
    const retained = readDraftRecovery(PUBLIC_TRUST.id)!;
    expect(retained.owner).toBe(PUBLIC_USER.id);
    expect(retained.confirmedDigest).toBe('approved-one');
    expect(retained.command?.key).toBeTruthy();
    expect(retained.command?.key).toBe(test.sent[0]?.key);
    expect(JSON.stringify(retained.command?.body)).toBe(test.sent[0]?.body);
    for (let attempt = 0; attempt < prefix.length; attempt++) {
      await clickStart(actor);
      await waitFor(() => expect(screen.getByRole('button', { name: 'Try again' })).toBeEnabled());
      expect(readDraftRecovery(PUBLIC_TRUST.id)).toEqual(retained);
    }
    await waitFor(() => expect(test.completed).toHaveBeenCalledWith(PUBLIC_TRUST));
    expect(test.client.requests.read).toHaveBeenCalledExactlyOnceWith(PUBLIC_TRUST.id);
    expect(test.sent).toHaveLength(prefix.length + 1);
    expect(screen.getByRole('checkbox')).toBeChecked();
    expect(screen.getByRole('checkbox')).toBeDisabled();
    expect(readDraftRecovery(PUBLIC_TRUST.id)).toEqual(retained);
    view.unmount();
    test.mount();
    await screen.findByRole('region', { name: 'CMA sign-in required' });
    expect(screen.getByRole('button', { name: 'Try again' })).toBeDisabled();
    expect(screen.getByRole('checkbox')).toBeChecked();
    expect(screen.getByRole('checkbox')).toBeDisabled();
    expect(readDraftRecovery(PUBLIC_TRUST.id)).toEqual(retained);
    await checkAccount(actor);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Try again' })).toBeEnabled());
    expect(test.sent).toHaveLength(prefix.length + 1);
    expect(readDraftRecovery(PUBLIC_TRUST.id)).toEqual(retained);
    await clickStart(actor);
    await waitFor(() => expect(test.completed).toHaveBeenLastCalledWith(PUBLIC_RECORD));
    expect(test.sent).toHaveLength(prefix.length + 2);
    for (const command of test.sent) expect(command).toEqual(test.sent[0]);
    expect(readDraftRecovery(PUBLIC_TRUST.id)).toBeUndefined();
    expect(test.client.requests.createOrResumeBound).not.toHaveBeenCalled();
    expect(test.client.requests.cancel).not.toHaveBeenCalled();
  },
);
