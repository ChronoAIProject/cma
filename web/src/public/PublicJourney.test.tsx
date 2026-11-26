import { StrictMode } from 'react';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { AuthContextProvider } from '../auth/context';
import type { AuthProvider, AuthState } from '../auth/types';
import { PUBLIC_PREVIEW, PUBLIC_RECORD, PUBLIC_USER, publicClient } from '../test/public';
import { PublicJourney } from './PublicJourney';
import { createIntent, readIntent } from './intent';

const route = {
  kind: 'public_trigger',
  publicId: 'public-example',
  placementId: 'plc_readme',
} as const;
const path = '/t/public-example/plc_readme';
const signedIn: AuthState = { status: 'authenticated', user: PUBLIC_USER };

function auth(initial: AuthState, returned: AuthState = initial): AuthProvider {
  let state = initial;
  return {
    current: vi.fn(async () => state),
    signIn: vi.fn(async () => {
      state = returned;
    }),
    signOut: vi.fn(async () => undefined),
  };
}
function setup(provider = auth(signedIn), client = publicClient()) {
  const navigate = vi.fn();
  const view = render(
    <StrictMode>
      <AuthContextProvider provider={provider}>
        <PublicJourney client={client} route={route} onNavigate={navigate} />
      </AuthContextProvider>
    </StrictMode>,
  );
  return { ...view, provider, client, navigate, user: userEvent.setup() };
}
beforeEach(() => vi.stubEnv('VITE_VISITOR_ALLOWANCE_GUIDANCE_ENABLED', 'true'));
afterEach(() => {
  vi.unstubAllEnvs();
  sessionStorage.clear();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('public visitor admission', () => {
  it('consumes a verified external click once and admits a no-review Trigger without another button', async () => {
    const client = publicClient();
    vi.mocked(client.requests.preview).mockResolvedValue({
      ...PUBLIC_PREVIEW,
      always_review: false,
    });
    vi.mocked(client.requests.entryIntent).mockResolvedValue({
      accepted: true,
      placement_id: route.placementId,
    });
    const test = setup(auth(signedIn), client);
    await waitFor(() => expect(test.client.requests.createOrResumeBound).toHaveBeenCalledOnce());
    expect(client.requests.entryIntent).toHaveBeenCalledOnce();
    expect(test.navigate).toHaveBeenCalledWith(
      { kind: 'launch', launchId: PUBLIC_RECORD.id },
      { replace: true },
    );
    expect(screen.queryByRole('button', { name: 'Open in CMA' })).not.toBeInTheDocument();
  });

  it('carries the external gesture through necessary anonymous sign-in with no extra confirmation', async () => {
    const client = publicClient();
    vi.mocked(client.requests.preview).mockResolvedValue({
      ...PUBLIC_PREVIEW,
      always_review: false,
    });
    vi.mocked(client.requests.entryIntent).mockResolvedValue({
      accepted: true,
      placement_id: route.placementId,
    });
    const test = setup(auth({ status: 'anonymous' }, signedIn), client);
    await waitFor(() => expect(client.requests.createOrResumeBound).toHaveBeenCalledOnce());
    expect(test.provider.signIn).toHaveBeenCalledOnce();
    expect(test.provider.signIn).toHaveBeenCalledWith(path, undefined);
    expect(client.requests.entryIntent).toHaveBeenCalledOnce();
  });

  it.each([{ accepted: false }, { accepted: true, placement_id: 'different-placement' }])(
    'does not admit no-review navigation without matching gesture evidence: %j',
    async (entry) => {
      const client = publicClient();
      vi.mocked(client.requests.preview).mockResolvedValue({
        ...PUBLIC_PREVIEW,
        always_review: false,
      });
      vi.mocked(client.requests.entryIntent).mockResolvedValue(entry);
      setup(auth(signedIn), client);
      await waitFor(() => expect(client.requests.entryIntent).toHaveBeenCalledOnce());
      expect(client.requests.createOrResumeBound).not.toHaveBeenCalled();
      expect(await screen.findByRole('button', { name: 'Open in CMA' })).toBeInTheDocument();
    },
  );

  it('preserves configured review even when an entry endpoint could return evidence', async () => {
    const client = publicClient();
    vi.mocked(client.requests.preview).mockResolvedValue({
      ...PUBLIC_PREVIEW,
      always_review: true,
    });
    vi.mocked(client.requests.entryIntent).mockResolvedValue({
      accepted: true,
      placement_id: route.placementId,
    });
    setup(auth(signedIn), client);
    expect(await screen.findByRole('button', { name: 'Open in CMA' })).toBeInTheDocument();
    expect(client.requests.entryIntent).not.toHaveBeenCalled();
    expect(client.requests.createOrResumeBound).not.toHaveBeenCalled();
  });

  it('does not start work from a GET or a signed-in session without a button intent', async () => {
    const test = setup();
    expect(await screen.findByRole('heading', { name: PUBLIC_PREVIEW.title })).toBeInTheDocument();
    expect(test.client.requests.createOrResumeBound).not.toHaveBeenCalled();
  });

  it('saves explicit anonymous intent, signs in, and continues once with the exact body and key', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(
        async () =>
          new Response(
            JSON.stringify({
              social_providers: ['github'],
              email_signup: false,
              invite_code_required: false,
              register_available: true,
            }),
          ),
      ),
    );
    const test = setup(auth({ status: 'anonymous' }, signedIn));
    await test.user.click(await screen.findByRole('button', { name: 'Open in CMA' }));
    expect(test.provider.signIn).toHaveBeenCalledWith(path, undefined);
    await waitFor(() => expect(test.client.requests.createOrResumeBound).toHaveBeenCalledOnce());
    expect(test.client.requests.createOrResumeBound).toHaveBeenCalledWith(
      'public-example',
      {
        placement_id: 'plc_readme',
        referrer_origin: undefined,
        start_if_ready: true,
        choice: undefined,
      },
      { idempotencyKey: expect.any(String), expectedSubject: PUBLIC_USER.id },
    );
    expect(test.navigate).toHaveBeenCalledWith(
      { kind: 'launch', launchId: PUBLIC_RECORD.id },
      { replace: true },
    );
    expect(readIntent(path)).toBeUndefined();
  });

  it('uses registration only from the create-account gesture and preserves publisher invitation guidance', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(
        async () =>
          new Response(
            JSON.stringify({
              social_providers: ['github', 'google'],
              email_signup: false,
              invite_code_required: true,
              register_available: true,
            }),
          ),
      ),
    );
    const client = publicClient();
    vi.mocked(client.requests.preview).mockResolvedValue({
      ...PUBLIC_PREVIEW,
      signup_hint: 'Ask your project owner for an invitation.',
    });
    const test = setup(auth({ status: 'anonymous' }), client);
    await test.user.click(await screen.findByRole('button', { name: 'Create account' }));
    expect(test.provider.signIn).toHaveBeenCalledWith(path, { screen: 'register' });
    expect(readIntent(path)).toMatchObject({ submitted: false, publicId: route.publicId });
    expect(screen.getByText('Ask your project owner for an invitation.')).toBeInTheDocument();
    expect(client.requests.createOrResumeBound).not.toHaveBeenCalled();
  });

  it('returns from OAuth with a saved intent and makes only one POST under StrictMode', async () => {
    const intent = createIntent(route);
    const test = setup();
    await waitFor(() => expect(test.client.requests.createOrResumeBound).toHaveBeenCalledOnce());
    expect(test.client.requests.createOrResumeBound).toHaveBeenCalledWith(
      route.publicId,
      intent.body,
      {
        idempotencyKey: intent.key,
        expectedSubject: PUBLIC_USER.id,
      },
    );
  });

  it('clears the saved intent after a settled quota refusal and starts again with a fresh key', async () => {
    const resetAt = '2026-11-13T00:00:00Z';
    const intent = createIntent(route);
    const client = publicClient();
    vi.mocked(client.requests.createOrResumeBound)
      .mockRejectedValueOnce(
        new ApiError(429, 'unsafe Launch resource_quota', 'resource_quota', undefined, undefined, {
          resource: 'visitor_launches',
          limit: 5,
          reset_at: resetAt,
        }),
      )
      .mockResolvedValue(PUBLIC_RECORD);
    const test = setup(auth(signedIn), client);
    const formatted = new Intl.DateTimeFormat(undefined, {
      month: 'short',
      day: 'numeric',
      hour: 'numeric',
      minute: '2-digit',
    }).format(new Date(resetAt));

    expect(await screen.findByRole('alert')).toHaveTextContent(formatted);
    expect(readIntent(path)).toBeUndefined();
    expect(client.requests.createOrResumeBound).toHaveBeenCalledOnce();
    expect(test.navigate).not.toHaveBeenCalled();

    await test.user.click(screen.getByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(test.navigate).toHaveBeenCalled());
    const calls = vi.mocked(client.requests.createOrResumeBound).mock.calls;
    expect(calls).toHaveLength(2);
    expect(calls[0]?.[2].idempotencyKey).toBe(intent.key);
    expect(calls[1]?.[2].idempotencyKey).not.toBe(intent.key);
    expect(calls[1]?.[1]).toEqual(intent.body);
  });

  it('discards a credential recovery intent after a settled quota refusal without automatically resubmitting', async () => {
    const client = publicClient();
    vi.mocked(client.requests.createOrResumeBound)
      .mockRejectedValueOnce(
        new ApiError(409, 'Sign in again.', 'sandbox_credential_setup_required'),
      )
      .mockRejectedValueOnce(
        new ApiError(429, 'Daily allowance used.', 'resource_quota', undefined, undefined, {
          resource: 'visitor_launches',
          limit: 5,
          reset_at: '2026-11-14T00:00:00Z',
        }),
      )
      .mockResolvedValue(PUBLIC_RECORD);
    const popup = { opener: {}, location: { href: '' }, close: vi.fn() };
    vi.spyOn(window, 'open').mockReturnValue(popup as unknown as Window);
    vi.stubGlobal(
      'fetch',
      vi.fn(
        async () =>
          new Response(
            JSON.stringify({
              authenticated: true,
              subject: PUBLIC_USER.id,
              visitor: true,
              capabilities: PUBLIC_USER.capabilities,
            }),
            { headers: { 'Content-Type': 'application/json' } },
          ),
      ),
    );
    const test = setup(auth(signedIn), client);

    await test.user.click(await screen.findByRole('button', { name: 'Open in CMA' }));
    await test.user.click(await screen.findByRole('button', { name: 'Sign in to CMA' }));
    await test.user.click(screen.getByRole('button', { name: 'Check signed-in account' }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Open in CMA' })).toBeEnabled());
    expect(client.requests.createOrResumeBound).toHaveBeenCalledOnce();

    await test.user.click(screen.getByRole('button', { name: 'Open in CMA' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('daily allowance');
    expect(readIntent(path)).toBeUndefined();
    await act(async () => undefined);
    expect(client.requests.createOrResumeBound).toHaveBeenCalledTimes(2);
    expect(test.navigate).not.toHaveBeenCalled();

    await test.user.click(screen.getByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(test.navigate).toHaveBeenCalledOnce());
    const calls = vi.mocked(client.requests.createOrResumeBound).mock.calls;
    expect(calls).toHaveLength(3);
    expect(calls[1]?.[2].idempotencyKey).not.toBe(calls[0]?.[2].idempotencyKey);
    expect(calls[2]?.[2].idempotencyKey).not.toBe(calls[1]?.[2].idempotencyKey);
    expect(calls[2]?.[1]).toEqual(calls[1]?.[1]);
  });

  it.each(['saved intent', 'pending entry check'] as const)(
    'does not reuse entry evidence after a settled refusal with %s',
    async (source) => {
      if (source === 'saved intent') createIntent(route);
      const client = publicClient();
      vi.mocked(client.requests.preview).mockResolvedValue({
        ...PUBLIC_PREVIEW,
        always_review: false,
      });
      let settleEntry!: (entry: { accepted: true; placement_id: string }) => void;
      const entry = new Promise<{ accepted: true; placement_id: string }>((resolve) => {
        settleEntry = resolve;
      });
      vi.mocked(client.requests.entryIntent).mockReturnValue(entry);
      vi.mocked(client.requests.createOrResumeBound)
        .mockRejectedValueOnce(
          new ApiError(429, 'Daily allowance used.', 'resource_quota', undefined, undefined, {
            resource: 'visitor_launches',
            reset_at: '2026-11-14T00:00:00Z',
          }),
        )
        .mockResolvedValue(PUBLIC_RECORD);
      const test = setup(auth(signedIn), client);
      if (source === 'pending entry check') {
        await waitFor(() => expect(client.requests.entryIntent).toHaveBeenCalledOnce());
        await test.user.click(screen.getByRole('button', { name: 'Open in CMA' }));
      }

      expect(await screen.findByRole('alert')).toHaveTextContent('daily allowance');
      await act(async () => settleEntry({ accepted: true, placement_id: route.placementId }));
      expect(client.requests.createOrResumeBound).toHaveBeenCalledOnce();
      expect(readIntent(path)).toBeUndefined();
      expect(test.navigate).not.toHaveBeenCalled();

      await test.user.click(screen.getByRole('button', { name: 'Try again' }));
      await waitFor(() => expect(test.navigate).toHaveBeenCalledOnce());
      const calls = vi.mocked(client.requests.createOrResumeBound).mock.calls;
      expect(calls).toHaveLength(2);
      expect(calls[1]?.[2].idempotencyKey).not.toBe(calls[0]?.[2].idempotencyKey);
    },
  );

  it.each([
    [429, 'resource_quota', { resource: 'visitor_launches' }],
    [429, 'resource_quota', { resource: 'visitor_launches', reset_at: 'unknown' }],
    [429, 'resource_quota', { resource: 'sandboxes', reset_at: '2026-11-13T00:00:00Z' }],
    [429, 'rate_limited', { resource: 'visitor_launches', reset_at: '2026-11-13T00:00:00Z' }],
    [503, 'resource_quota', { resource: 'visitor_launches', reset_at: '2026-11-13T00:00:00Z' }],
  ])(
    'preserves the intent and key for an unsettled refusal: %i %s %j',
    async (status, code, extensions) => {
      const intent = createIntent(route);
      const client = publicClient();
      vi.mocked(client.requests.createOrResumeBound)
        .mockRejectedValueOnce(
          new ApiError(status, 'Unknown', code, undefined, undefined, extensions),
        )
        .mockResolvedValue(PUBLIC_RECORD);
      const test = setup(auth(signedIn), client);

      expect(await screen.findByRole('alert')).toHaveTextContent('Your previous choices are saved');
      expect(readIntent(path)).toEqual({
        ...intent,
        owner: PUBLIC_USER.id,
        submitted: true,
        wireBody: JSON.stringify(intent.body),
      });
      expect(test.navigate).not.toHaveBeenCalled();

      await test.user.click(screen.getByRole('button', { name: 'Try again' }));
      await waitFor(() => expect(test.navigate).toHaveBeenCalled());
      const calls = vi.mocked(client.requests.createOrResumeBound).mock.calls;
      expect(calls).toHaveLength(2);
      expect(calls[1]).toEqual(calls[0]);
    },
  );

  it.each(['resume', 'new'] as const)(
    'submits the explicit repeat %s choice with a fresh key',
    async (choice) => {
      const client = publicClient();
      vi.mocked(client.requests.createOrResumeBound)
        .mockRejectedValueOnce(
          new ApiError(409, 'Choose', 'trigger_repeat_choice_required', undefined, undefined, {
            existing: {
              agent_id: 'agt_previous',
              title: 'Previous research',
              created_at: PUBLIC_RECORD.created_at,
              first_message_excerpt: 'Find the test failures',
            },
          }),
        )
        .mockResolvedValue(PUBLIC_RECORD);
      const test = setup(auth(signedIn), client);
      await test.user.click(await screen.findByRole('button', { name: 'Open in CMA' }));
      await test.user.click(
        await screen.findByRole('button', {
          name: choice === 'resume' ? 'Continue where you left off' : 'Start fresh',
        }),
      );
      await waitFor(() => expect(test.navigate).toHaveBeenCalled());
      const calls = vi.mocked(client.requests.createOrResumeBound).mock.calls;
      expect(calls).toHaveLength(2);
      expect(calls[1]?.[1].choice).toBe(choice);
      expect(calls[1]?.[2].idempotencyKey).not.toBe(calls[0]?.[2].idempotencyKey);
    },
  );

  it.each(['transport', 'idempotency_in_flight', 'idempotency_recovery_required'])(
    'preserves body and key after uncertain admission %s',
    async (code) => {
      const client = publicClient();
      vi.mocked(client.requests.createOrResumeBound)
        .mockRejectedValueOnce(
          code === 'transport' ? new Error('offline') : new ApiError(409, 'Unknown', code),
        )
        .mockResolvedValue(PUBLIC_RECORD);
      const test = setup(auth(signedIn), client);
      await test.user.click(await screen.findByRole('button', { name: 'Open in CMA' }));
      await test.user.click(await screen.findByRole('button', { name: 'Try again' }));
      await waitFor(() => expect(test.navigate).toHaveBeenCalled());
      const calls = vi.mocked(client.requests.createOrResumeBound).mock.calls;
      expect(calls[1]).toEqual(calls[0]);
    },
  );

  it('renders retired targets without a start button, including a previously saved intent', async () => {
    createIntent(route);
    const client = publicClient();
    vi.mocked(client.requests.preview).mockResolvedValue({
      ...PUBLIC_PREVIEW,
      availability: 'retired',
    });
    setup(auth(signedIn), client);
    expect(
      await screen.findByText('This agent is no longer available from this page.'),
    ).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Open in CMA' })).not.toBeInTheDocument();
    expect(client.requests.createOrResumeBound).not.toHaveBeenCalled();
  });

  it("does not auto-start expired intent or another viewer's submitted intent", async () => {
    const intent = createIntent(route);
    sessionStorage.setItem(
      'cma:public-intent',
      JSON.stringify({ ...intent, owner: 'another-viewer', submitted: true }),
    );
    const test = setup();
    await screen.findByRole('heading', { name: PUBLIC_PREVIEW.title });
    await act(async () => undefined);
    expect(test.client.requests.createOrResumeBound).not.toHaveBeenCalled();
  });
});
