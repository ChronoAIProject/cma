import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, type Launch } from '@cma/client';
import { AuthContextProvider } from './context';
import type { AuthProvider } from './types';
import { needsSandboxCredentialSetup } from './credentialSetup';
import { NewSandboxCard } from '../create/NewSandboxCard';
import { PublicJourney } from '../public/PublicJourney';
import { ReviewCard } from '../public/ReviewCard';
import { createIntent, readIntent } from '../public/intent';
import { connectionClient } from '../connections/client';
import { readDraftRecovery } from '../public/draftRecovery';
import { agentClient } from '../test/agent';
import { creationClient, CREATION_SANDBOX } from '../test/creation';
import { sandboxFixture } from '../test/hierarchy';
import { PUBLIC_RECORD, PUBLIC_TRUST, PUBLIC_USER, publicClient } from '../test/public';

const refusal = () => new ApiError(409, 'Sign in again.', 'sandbox_credential_setup_required');
type Surface = 'sandbox' | 'landing' | 'review';

async function setup(
  surface: Surface,
  errors: unknown[] = [refusal()],
  options: { record?: Launch; client?: ReturnType<typeof publicClient>; actionName?: string } = {},
) {
  const command = vi.fn();
  for (const error of errors) command.mockRejectedValueOnce(error);
  command.mockResolvedValue(surface === 'sandbox' ? sandboxFixture() : PUBLIC_RECORD);
  const completed = vi.fn();
  let authenticatedSubject = PUBLIC_USER.id;
  const provider: AuthProvider = {
    current: async () => ({
      status: 'authenticated',
      user: { ...PUBLIC_USER, id: authenticatedSubject },
    }),
    signIn: vi.fn(),
    signOut: vi.fn(),
  };
  const client = options.client ?? publicClient();
  const popup = { opener: {}, location: { href: '' }, close: vi.fn() };
  const open = vi.spyOn(window, 'open').mockReturnValue(popup as unknown as Window);
  let subject = PUBLIC_USER.id;
  const session = vi.fn(
    async () =>
      new Response(
        JSON.stringify({
          authenticated: true,
          subject,
          visitor: true,
          capabilities: ['launch_triggers'],
        }),
        { headers: { 'Content-Type': 'application/json' } },
      ),
  );
  vi.stubGlobal('fetch', session);
  const mount = () =>
    render(
      <AuthContextProvider provider={provider}>
        {surface === 'sandbox' ? (
          <NewSandboxCard
            client={agentClient({ createSandbox: command })}
            catalog={creationClient()}
            initial={{ ...CREATION_SANDBOX, requiresConfirmation: false }}
            onCreated={completed}
            onClose={vi.fn()}
          />
        ) : surface === 'landing' ? (
          <PublicJourney
            client={{ requests: { ...client.requests, createOrResumeBound: command } }}
            route={{
              kind: 'public_trigger',
              publicId: 'public-example',
              placementId: 'plc_readme',
            }}
            onNavigate={completed}
          />
        ) : (
          <ReviewCard
            client={{ requests: { ...client.requests, start: command } }}
            record={options.record ?? PUBLIC_TRUST}
            expectedSubject={authenticatedSubject}
            onChanged={completed}
            onClose={vi.fn()}
          />
        )}
      </AuthContextProvider>,
    );
  const view = mount();
  const actor = userEvent.setup();
  if (surface === 'review' && screen.queryByRole('checkbox'))
    await actor.click(screen.getByRole('checkbox'));
  if (surface === 'sandbox')
    fireEvent.change(screen.getByLabelText('Name (optional)'), {
      target: { value: 'Keep my draft' },
    });
  const name =
    surface === 'sandbox'
      ? 'Create sandbox'
      : surface === 'landing'
        ? 'Open in CMA'
        : (options.actionName ?? 'Continue');
  await actor.click(await screen.findByRole('button', { name }));
  await screen.findByText('Sign in to CMA with this same account before trying again.');
  return {
    ...view,
    mount,
    actor,
    command,
    completed,
    popup,
    open,
    session,
    name,
    setSubject: (value: string) => {
      subject = value;
    },
    setAuthenticatedSubject: (value: string) => {
      authenticatedSubject = value;
    },
  };
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  sessionStorage.clear();
});

for (const surface of ['sandbox', 'landing', 'review'] as const) {
  it(`${surface} keeps the draft and requires an explicit fresh-key retry after same-account checking`, async () => {
    const test = await setup(surface);
    expect(screen.getByRole('button', { name: test.name })).toBeDisabled();
    await test.actor.click(screen.getByRole('button', { name: 'Sign in to CMA' }));
    expect(test.popup.opener).toBeNull();
    const login = new URL(test.popup.location.href, window.location.origin);
    expect(login.pathname).toBe('/auth/login');
    expect([...login.searchParams]).toEqual([
      [
        'return_to',
        surface === 'sandbox'
          ? '/'
          : surface === 'landing'
            ? '/t/public-example/plc_readme'
            : '/t/public-example',
      ],
    ]);
    expect(test.command).toHaveBeenCalledOnce();
    await test.actor.click(screen.getByRole('button', { name: 'Check signed-in account' }));
    await waitFor(() => expect(screen.getByRole('button', { name: test.name })).toBeEnabled());
    expect(test.command).toHaveBeenCalledOnce();
    expect(test.session).toHaveBeenCalledWith(
      '/auth/session',
      expect.objectContaining({
        method: 'GET',
        credentials: 'include',
        redirect: 'error',
      }),
    );
    if (surface === 'sandbox')
      expect(screen.getByLabelText('Name (optional)')).toHaveValue('Keep my draft');
    if (surface === 'review') expect(screen.getByRole('checkbox')).toBeChecked();
    await test.actor.click(screen.getByRole('button', { name: test.name }));
    await waitFor(() => expect(test.completed).toHaveBeenCalled());
    const [first, second] = test.command.mock.calls;
    expect(second!.slice(0, -1)).toEqual(first!.slice(0, -1));
    expect(second!.at(-1).idempotencyKey).not.toBe(first!.at(-1).idempotencyKey);
    expect(test.popup.close).toHaveBeenCalled();
  });

  it(`${surface} keeps the fresh retry key if that later attempt loses its response`, async () => {
    const test = await setup(surface, [refusal(), new TypeError('response lost')]);
    await test.actor.click(screen.getByRole('button', { name: 'Sign in to CMA' }));
    await test.actor.click(screen.getByRole('button', { name: 'Check signed-in account' }));
    await waitFor(() => expect(screen.getByRole('button', { name: test.name })).toBeEnabled());
    await test.actor.click(screen.getByRole('button', { name: test.name }));
    const retry = await screen.findByRole('button', {
      name: surface === 'sandbox' ? 'Retry creation' : 'Try again',
    });
    expect(screen.queryByRole('region', { name: 'CMA sign-in required' })).not.toBeInTheDocument();
    await test.actor.click(retry);
    await waitFor(() => expect(test.completed).toHaveBeenCalled());
    const [first, second, third] = test.command.mock.calls;
    expect(second!.at(-1).idempotencyKey).not.toBe(first!.at(-1).idempotencyKey);
    expect(third!.at(-1).idempotencyKey).toBe(second!.at(-1).idempotencyKey);
    expect(third!.slice(0, -1)).toEqual(second!.slice(0, -1));
  });

  it(`${surface} refuses a different account and offers a safe popup-blocked sign-in link`, async () => {
    const test = await setup(surface);
    test.open.mockReturnValue(null);
    test.session.mockResolvedValue(
      new Response(
        JSON.stringify({
          authenticated: true,
          subject: 'different-owner',
          visitor: true,
          capabilities: [],
        }),
        { headers: { 'Content-Type': 'application/json' } },
      ),
    );
    await test.actor.click(screen.getByRole('button', { name: 'Sign in to CMA' }));
    const link = screen.getByRole('link', { name: 'Open CMA sign-in' });
    expect(link).toHaveAttribute('target', '_blank');
    expect(link).toHaveAttribute('rel', 'noopener noreferrer');
    expect(
      screen.queryByRole('button', { name: 'Check signed-in account' }),
    ).not.toBeInTheDocument();
    await test.actor.click(link);
    await test.actor.click(screen.getByRole('button', { name: 'Check signed-in account' }));
    await screen.findByText('Sign in with the same CMA account that made this request.');
    expect(screen.getByRole('button', { name: test.name })).toBeDisabled();
    expect(test.command).toHaveBeenCalledOnce();
    expect(test.completed).not.toHaveBeenCalled();
  });

  it.each(['fresh', 'ambiguous'] as const)(
    `${surface} rechecks live identity before a post-recovery %s retry and preserves the command`,
    async (phase) => {
      const test = await setup(
        surface,
        phase === 'fresh' ? [refusal()] : [refusal(), new TypeError('response lost')],
      );
      await test.actor.click(screen.getByRole('button', { name: 'Sign in to CMA' }));
      await test.actor.click(screen.getByRole('button', { name: 'Check signed-in account' }));
      await waitFor(() => expect(screen.getByRole('button', { name: test.name })).toBeEnabled());
      if (phase === 'ambiguous') {
        await test.actor.click(screen.getByRole('button', { name: test.name }));
        await screen.findByRole('button', {
          name: surface === 'sandbox' ? 'Retry creation' : 'Try again',
        });
      }
      const writes = test.command.mock.calls.length;
      test.setSubject('different-owner');
      await test.actor.click(
        screen.getByRole('button', {
          name:
            phase === 'fresh' ? test.name : surface === 'sandbox' ? 'Retry creation' : 'Try again',
        }),
      );
      await screen.findByText('Sign in with the same CMA account that made this request.');
      expect(test.command).toHaveBeenCalledTimes(writes);
      expect(test.completed).not.toHaveBeenCalled();
      const blockedIntent =
        surface === 'landing' ? readIntent('/t/public-example/plc_readme') : undefined;
      await test.actor.click(screen.getByRole('button', { name: 'Sign in to CMA' }));
      await test.actor.click(screen.getByRole('button', { name: 'Check signed-in account' }));
      expect(test.command).toHaveBeenCalledTimes(writes);
      test.setSubject(PUBLIC_USER.id);
      await test.actor.click(screen.getByRole('button', { name: 'Check signed-in account' }));
      const retryName =
        surface === 'sandbox' ? 'Retry creation' : surface === 'review' ? 'Try again' : test.name;
      await waitFor(() => expect(screen.getByRole('button', { name: retryName })).toBeEnabled());
      expect(test.command).toHaveBeenCalledTimes(writes);
      await test.actor.click(screen.getByRole('button', { name: retryName }));
      await waitFor(() => expect(test.completed).toHaveBeenCalledOnce());
      const final = test.command.mock.calls.at(-1)!;
      expect(final.slice(0, -1)).toEqual(test.command.mock.calls[0]!.slice(0, -1));
      if (phase === 'ambiguous')
        expect(final.at(-1).idempotencyKey).toBe(test.command.mock.calls[1]!.at(-1).idempotencyKey);
      else
        expect(final.at(-1).idempotencyKey).not.toBe(
          test.command.mock.calls[0]!.at(-1).idempotencyKey,
        );
      if (blockedIntent !== undefined) {
        expect(final.at(-1).idempotencyKey).toBe(blockedIntent.key);
        expect(final[1]).toEqual(blockedIntent.body);
        expect(blockedIntent.owner).toBe(PUBLIC_USER.id);
      }
    },
  );
}

it('keeps Start fresh choice, placement, referrer, owner and retry key through recovery and reload', async () => {
  const referrer = vi
    .spyOn(document, 'referrer', 'get')
    .mockReturnValue('https://publisher.example.test/readme');
  const original = createIntent(
    { kind: 'public_trigger', publicId: 'public-example', placementId: 'plc_readme' },
    'new',
  );
  const test = await setup('landing', [refusal(), new TypeError('response lost')]);
  await test.actor.click(screen.getByRole('button', { name: 'Sign in to CMA' }));
  await test.actor.click(screen.getByRole('button', { name: 'Check signed-in account' }));
  referrer.mockReturnValue('https://different.example.test/');
  expect(test.command).toHaveBeenCalledOnce();
  await test.actor.click(screen.getByRole('button', { name: test.name }));
  await screen.findByRole('button', { name: 'Try again' });
  const retained = readIntent(original.path)!;
  expect(retained).toEqual({
    ...original,
    key: expect.any(String),
    owner: PUBLIC_USER.id,
    submitted: true,
    wireBody: JSON.stringify(original.body),
    credentialRecovery: 'retry',
  });
  expect(retained.key).not.toBe(original.key);
  expect(retained.body).toMatchObject({
    choice: 'new',
    placement_id: 'plc_readme',
    referrer_origin: 'https://publisher.example.test',
  });
  test.unmount();
  test.setSubject('different-owner');
  test.mount();
  await screen.findByRole('region', { name: 'CMA sign-in required' });
  expect(test.command).toHaveBeenCalledTimes(2);
  await test.actor.click(screen.getByRole('button', { name: 'Sign in to CMA' }));
  await test.actor.click(screen.getByRole('button', { name: 'Check signed-in account' }));
  await screen.findByText('Sign in with the same CMA account that made this request.');
  expect(readIntent(original.path)).toEqual(retained);
  test.setSubject(PUBLIC_USER.id);
  await test.actor.click(screen.getByRole('button', { name: 'Check signed-in account' }));
  expect(test.command).toHaveBeenCalledTimes(2);
  await test.actor.click(screen.getByRole('button', { name: test.name }));
  await waitFor(() => expect(test.completed).toHaveBeenCalledOnce());
  expect(test.command.mock.calls[2]).toEqual(test.command.mock.calls[1]);
});

it.each(['same-tab', 'reload'] as const)(
  'requires explicit verification after %s recovery even when a pending hosted connection becomes satisfied',
  async (mode) => {
    const client = publicClient();
    const record: Launch = {
      ...PUBLIC_RECORD,
      state: 'draft',
      review: { can_start_now: false, needs: [{ type: 'connection', service: 'code' }] },
    };
    vi.mocked(client.requests.requirements).mockResolvedValue({
      services: [
        { service: 'code', state: 'missing', display_name: 'Code host', connection_mode: 'hosted' },
      ],
      checked_at: record.created_at,
    });
    const test = await setup('review', [refusal()], {
      record,
      client,
      actionName: 'Verify connections',
    });
    expect(readDraftRecovery(record.id)).toEqual({ owner: PUBLIC_USER.id });
    if (mode === 'reload') {
      test.unmount();
      test.setSubject('different-owner');
      test.setAuthenticatedSubject('different-owner');
      test.mount();
      await screen.findByRole('region', { name: 'CMA sign-in required' });
      await test.actor.click(screen.getByRole('button', { name: 'Sign in to CMA' }));
      await test.actor.click(screen.getByRole('button', { name: 'Check signed-in account' }));
      await screen.findByText('Sign in with the same CMA account that made this request.');
      expect(test.command).toHaveBeenCalledOnce();
      expect(readDraftRecovery(record.id)?.owner).toBe(PUBLIC_USER.id);
      test.setSubject(PUBLIC_USER.id);
    }
    await test.actor.click(screen.getByRole('button', { name: 'Sign in to CMA' }));
    await test.actor.click(screen.getByRole('button', { name: 'Check signed-in account' }));
    await waitFor(() => expect(connectionClient.check).toHaveBeenCalled());
    expect(screen.queryByRole('region', { name: 'Connect Code host' })).not.toBeInTheDocument();
    expect(test.command).toHaveBeenCalledOnce();
    expect(test.completed).not.toHaveBeenCalled();
    await test.actor.click(screen.getByRole('button', { name: test.name }));
    await waitFor(() => expect(test.completed).toHaveBeenCalledOnce());
    const [first, second] = test.command.mock.calls;
    expect(second!.at(-1).idempotencyKey).not.toBe(first!.at(-1).idempotencyKey);
    expect(second!.slice(0, -1)).toEqual(first!.slice(0, -1));
    expect(readDraftRecovery(record.id)).toBeUndefined();
  },
);

it('keeps the original Draft subject, trust and ambiguous Start packet through reload', async () => {
  const test = await setup('review', [refusal(), new TypeError('response lost')]);
  await test.actor.click(screen.getByRole('button', { name: 'Sign in to CMA' }));
  await test.actor.click(screen.getByRole('button', { name: 'Check signed-in account' }));
  await test.actor.click(screen.getByRole('button', { name: 'Continue' }));
  await screen.findByRole('button', { name: 'Try again' });
  const retained = readDraftRecovery(PUBLIC_TRUST.id)!;
  expect(retained).toEqual({
    owner: PUBLIC_USER.id,
    confirmedDigest: 'approved-one',
    command: {
      key: test.command.mock.calls[1]![2].idempotencyKey,
      body: test.command.mock.calls[1]![1],
    },
  });
  test.unmount();
  test.setSubject('different-owner');
  test.setAuthenticatedSubject('different-owner');
  test.mount();
  await screen.findByRole('region', { name: 'CMA sign-in required' });
  expect(test.command).toHaveBeenCalledTimes(2);
  expect(screen.getByRole('checkbox')).toBeChecked();
  expect(screen.getByRole('checkbox')).toBeDisabled();
  await test.actor.click(screen.getByRole('button', { name: 'Sign in to CMA' }));
  await test.actor.click(screen.getByRole('button', { name: 'Check signed-in account' }));
  await screen.findByText('Sign in with the same CMA account that made this request.');
  expect(readDraftRecovery(PUBLIC_TRUST.id)).toEqual(retained);
  test.setSubject(PUBLIC_USER.id);
  await test.actor.click(screen.getByRole('button', { name: 'Check signed-in account' }));
  expect(test.command).toHaveBeenCalledTimes(2);
  await test.actor.click(screen.getByRole('button', { name: 'Try again' }));
  await waitFor(() => expect(test.completed).toHaveBeenCalledOnce());
  expect(test.command.mock.calls[2]).toEqual(test.command.mock.calls[1]);
  expect(readDraftRecovery(PUBLIC_TRUST.id)).toBeUndefined();
});

it('keeps credential recovery available when identity checking is unavailable', async () => {
  const test = await setup('sandbox');
  test.session.mockRejectedValueOnce(new TypeError('identity unavailable'));
  await test.actor.click(screen.getByRole('button', { name: 'Sign in to CMA' }));
  await test.actor.click(screen.getByRole('button', { name: 'Check signed-in account' }));
  await screen.findByText('Your CMA account could not be checked. Try again.');
  expect(screen.getByRole('button', { name: 'Create sandbox' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Check signed-in account' })).toBeEnabled();
  expect(test.command).toHaveBeenCalledOnce();
});

it('does not treat the same-account check as proof that the next creation must be accepted', async () => {
  const test = await setup('sandbox', [refusal(), refusal()]);
  await test.actor.click(screen.getByRole('button', { name: 'Sign in to CMA' }));
  await test.actor.click(screen.getByRole('button', { name: 'Check signed-in account' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Create sandbox' })).toBeEnabled());
  await test.actor.click(screen.getByRole('button', { name: 'Create sandbox' }));
  await screen.findByRole('region', { name: 'CMA sign-in required' });
  expect(test.completed).not.toHaveBeenCalled();
  expect(screen.getByLabelText('Name (optional)')).toHaveValue('Keep my draft');
  await act(async () => undefined);
  expect(test.command).toHaveBeenCalledTimes(2);
});

it('only recognizes the typed, definitive 409 credential refusal', () => {
  expect(needsSandboxCredentialSetup(refusal())).toBe(true);
  for (const error of [
    new TypeError('lost response'),
    new ApiError(503, 'unavailable', 'sandbox_credential_setup_required'),
    new ApiError(409, 'pending', 'idempotency_in_flight'),
    { status: 409, code: 'sandbox_credential_setup_required' },
  ])
    expect(needsSandboxCredentialSetup(error)).toBe(false);
});
import '../test/connectedAccounts';
