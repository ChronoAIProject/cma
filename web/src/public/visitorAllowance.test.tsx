import { StrictMode } from 'react';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ApiError, TransportError } from '@cma/client';
import { AuthContextProvider } from '../auth/context';
import type { AuthProvider } from '../auth/types';
import { PUBLIC_RECORD, PUBLIC_USER, publicClient } from '../test/public';
import { failureRemediation, refusalSentence } from '../vocabulary/errors';
import { isUnsettledCommand } from './admission';
import { createIntent, readIntent, writeIntent } from './intent';
import { PublicJourney } from './PublicJourney';

const flag = 'VITE_VISITOR_ALLOWANCE_GUIDANCE_ENABLED';
const route = {
  kind: 'public_trigger',
  publicId: 'public-example',
  placementId: 'plc_readme',
} as const;
const path = '/t/public-example/plc_readme';
const resetAt = '2026-11-21T00:00:00Z';
const extensions = { resource: 'visitor_launches', reset_at: resetAt };
const settled = () =>
  new ApiError(429, 'Allowance used.', 'resource_quota', undefined, undefined, extensions);
const genericRefusal =
  'CMA could not confirm the result. Your previous choices are saved; try again in a moment.';
const genericFailure =
  'CMA could not finish preparing your agent. Check again, or share the support code with your administrator.';

function mount(client: ReturnType<typeof publicClient>) {
  const provider: AuthProvider = {
    current: vi.fn(async () => ({ status: 'authenticated' as const, user: PUBLIC_USER })),
    signIn: vi.fn(),
    signOut: vi.fn(),
  };
  const navigate = vi.fn();
  const view = render(
    <StrictMode>
      <AuthContextProvider provider={provider}>
        <PublicJourney client={client} route={route} onNavigate={navigate} />
      </AuthContextProvider>
    </StrictMode>,
  );
  return { ...view, navigate, user: userEvent.setup() };
}

function retain(recovery: boolean) {
  const original = createIntent(route);
  const intent = {
    ...original,
    wireBody: JSON.stringify(original.body),
    owner: PUBLIC_USER.id,
    submitted: true,
    ...(recovery ? { credentialRecovery: 'retry' as const } : {}),
  };
  writeIntent(intent);
  return intent;
}

async function recover(test: ReturnType<typeof mount>) {
  await test.user.click(await screen.findByRole('button', { name: 'Sign in to CMA' }));
  await test.user.click(screen.getByRole('button', { name: 'Check signed-in account' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Open in CMA' })).toBeEnabled());
  await test.user.click(screen.getByRole('button', { name: 'Open in CMA' }));
}

beforeEach(() => {
  vi.spyOn(window, 'open').mockReturnValue({
    opener: {},
    location: { href: '' },
    close: vi.fn(),
  } as unknown as Window);
  vi.stubEnv(flag, 'true');
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
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
});

it.each([undefined, 'false', '', 'TRUE', '1'])(
  'preserves baseline classification and copy when the switch is %j',
  (value) => {
    vi.stubEnv(flag, value);
    expect(isUnsettledCommand(settled())).toBe(true);
    expect(refusalSentence(settled())).toBe(genericRefusal);
    expect(failureRemediation('sandbox_limit_reached')).toBe(genericFailure);
  },
);

describe.each([false, true])('credential recovery: %s', (recovery) => {
  it.each([undefined, 'false', 'true'])(
    'changes a settled request key only with the switch enabled (%j) and an explicit click',
    async (value) => {
      vi.stubEnv(flag, value);
      const intent = retain(recovery);
      const client = publicClient();
      vi.mocked(client.requests.createOrResumeBound)
        .mockRejectedValueOnce(settled())
        .mockResolvedValue(PUBLIC_RECORD);
      const test = mount(client);
      if (recovery) await recover(test);
      expect(await screen.findByRole('alert')).toHaveTextContent(
        value === 'true' ? 'daily allowance' : genericRefusal,
      );
      expect(readIntent(path)).toEqual(value === 'true' ? undefined : intent);
      await act(async () => undefined);
      expect(client.requests.createOrResumeBound).toHaveBeenCalledOnce();
      await test.user.click(screen.getByRole('button', { name: 'Try again' }));
      await waitFor(() => expect(test.navigate).toHaveBeenCalledOnce());
      const calls = vi.mocked(client.requests.createOrResumeBound).mock.calls;
      expect(calls).toHaveLength(2);
      expect(calls[0]?.[2].idempotencyKey).toBe(intent.key);
      expect(calls[1]?.[1]).toEqual(intent.body);
      if (value === 'true') expect(calls[1]?.[2].idempotencyKey).not.toBe(intent.key);
      else expect(calls[1]).toEqual(calls[0]);
    },
  );

  it.each([
    new TransportError('Request timed out'),
    new ApiError(503, 'Unavailable', 'resource_quota', undefined, undefined, extensions),
    new ApiError(429, 'Busy', 'rate_limited', undefined, undefined, extensions),
    new ApiError(429, 'Unknown', 'resource_quota', undefined, undefined, {
      resource: 'visitor_launches',
    }),
    new ApiError(429, 'Unknown', 'resource_quota', undefined, undefined, {
      ...extensions,
      reset_at: 'invalid',
    }),
  ])('keeps the exact uncertain command after same-owner reload: %s', async (error) => {
    const intent = retain(recovery);
    const client = publicClient();
    vi.mocked(client.requests.createOrResumeBound)
      .mockRejectedValueOnce(error)
      .mockResolvedValue(PUBLIC_RECORD);
    const first = mount(client);
    if (recovery) await recover(first);
    expect(await screen.findByRole('alert')).toHaveTextContent(genericRefusal);
    expect(readIntent(path)).toEqual(intent);
    first.unmount();
    const second = mount(client);
    if (recovery) await recover(second);
    await waitFor(() => expect(second.navigate).toHaveBeenCalledOnce());
    const calls = vi.mocked(client.requests.createOrResumeBound).mock.calls;
    expect(calls).toHaveLength(2);
    expect(calls[1]).toEqual(calls[0]);
    expect(calls[1]?.[2].idempotencyKey).toBe(intent.key);
  });
});

it('crosses the UTC reset and reloads without starting work until a new explicit action', async () => {
  vi.useFakeTimers({ toFake: ['Date'] });
  vi.setSystemTime(new Date('2026-11-20T23:59:50Z'));
  const intent = retain(false);
  const client = publicClient();
  vi.mocked(client.requests.createOrResumeBound)
    .mockRejectedValueOnce(settled())
    .mockResolvedValue(PUBLIC_RECORD);
  const first = mount(client);
  expect(await screen.findByRole('alert')).toHaveTextContent(
    new Intl.DateTimeFormat(undefined, {
      month: 'short',
      day: 'numeric',
      hour: 'numeric',
      minute: '2-digit',
    }).format(new Date(resetAt)),
  );
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-11-20T23:59:50Z'));
  await act(async () => {
    await vi.advanceTimersByTimeAsync(20_000);
  });
  expect(client.requests.createOrResumeBound).toHaveBeenCalledOnce();
  expect(first.navigate).not.toHaveBeenCalled();
  first.unmount();
  vi.useRealTimers();
  vi.useFakeTimers({ toFake: ['Date'] });
  vi.setSystemTime(new Date('2026-11-21T00:00:10Z'));
  const second = mount(client);
  const button = await screen.findByRole('button', { name: 'Open in CMA' });
  expect(client.requests.createOrResumeBound).toHaveBeenCalledOnce();
  expect(readIntent(path)).toBeUndefined();
  await second.user.click(button);
  await waitFor(() => expect(second.navigate).toHaveBeenCalledOnce());
  expect(vi.mocked(client.requests.createOrResumeBound).mock.calls[1]?.[2].idempotencyKey).not.toBe(
    intent.key,
  );
});

it('does not send a replacement when saving the new explicit intent fails', async () => {
  retain(false);
  const client = publicClient();
  vi.mocked(client.requests.createOrResumeBound)
    .mockRejectedValueOnce(settled())
    .mockResolvedValue(PUBLIC_RECORD);
  const test = mount(client);
  expect(await screen.findByRole('alert')).toHaveTextContent('daily allowance');
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new Error('Storage unavailable');
  });
  await test.user.click(screen.getByRole('button', { name: 'Try again' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('could not save');
  expect(client.requests.createOrResumeBound).toHaveBeenCalledOnce();
  expect(readIntent(path)).toBeUndefined();
  expect(test.navigate).not.toHaveBeenCalled();
});
