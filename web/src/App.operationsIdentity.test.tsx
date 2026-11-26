import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, createClient } from '@cma/client';
import { App } from './App';
import { CmaClientProvider } from './api/context';
import { createRestWebClient } from './api/rest';
import { useSandboxCapacity } from './sandbox/useSandboxCapacity';
import type { CmaWebClient } from './api/client';
import { AuthContextProvider, useAuth } from './auth/context';
import type { AuthProvider, AuthState } from './auth/types';
import { agentClient } from './test/agent';
import { reportFixture, metricRow } from './usage/metrics/testFixtures';
import { operatorClient } from './operator/testFixtures';
import './test/connectedAccounts';

vi.mock('./usage/metrics/MetricChart', () => ({ MetricChart: () => <p>Measured chart</p> }));
afterEach(() => window.history.replaceState(null, '', '/'));

function RecheckIdentity() {
  const { retry } = useAuth();
  return <button onClick={retry}>Recheck test identity</button>;
}
const alice = (): AuthState => ({
  status: 'authenticated',
  user: {
    id: 'realm:alice',
    handle: 'Alice',
    capabilities: ['read_all_sandboxes', 'read_activity_log'],
    visitor: false,
  },
});

it.each(['account', 'sign-in epoch', 'capability'])(
  'keeps Operations reports isolated through the Console after an actual App %s transition',
  async (transition) => {
    window.history.replaceState(null, '', '/operations');
    let identity = alice();
    const provider: AuthProvider = {
      current: vi.fn(async () => identity),
      signIn: vi.fn(async () => {}),
      signOut: vi.fn(async () => {}),
    };
    const operator = operatorClient();
    const metrics = operator.adminUsageMetrics;
    metrics.create.mockResolvedValue({
      ...reportFixture(true),
      scope: { principal: 'realm:alice', admin: true },
    });
    metrics.sandboxes.mockResolvedValue({
      report_id: 'report-one',
      as_of: reportFixture(true).as_of,
      items: [{ ...metricRow, name: 'Private Alice Sandbox' }],
      next: null,
      total_count: 1,
    });
    const client = agentClient();
    render(
      <AuthContextProvider provider={provider}>
        <CmaClientProvider client={client} operator={operator}>
          <RecheckIdentity />
          <App />
        </CmaClientProvider>
      </AuthContextProvider>,
    );
    // Lazy route imports can queue behind the full suite's parallel transforms.
    // This setup wait is not a product latency assertion.
    await screen.findByRole('button', { name: 'Private Alice Sandbox' }, { timeout: 5000 });
    let resolveLate!: (value: ReturnType<typeof reportFixture>) => void;
    metrics.create.mockReturnValueOnce(
      new Promise((resolve) => {
        resolveLate = resolve;
      }),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Refresh usage' }));
    await waitFor(() => expect(resolveLate).toBeDefined());
    const staleSignal = metrics.create.mock.calls.at(-1)![1].signal as AbortSignal;
    const next = alice();
    if (next.status !== 'authenticated') throw new Error('Expected authenticated fixture');
    if (transition === 'account') identity = { ...next, user: { ...next.user, id: 'realm:bob' } };
    else if (transition === 'capability')
      identity = { ...next, user: { ...next.user, capabilities: ['read_all_sandboxes'] } };
    else identity = { status: 'anonymous' };
    metrics.create.mockRejectedValue(
      new ApiError(transition === 'capability' ? 403 : 503, 'Refused'),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Recheck test identity' }));
    await waitFor(() => expect(staleSignal.aborted).toBe(true));
    expect(screen.queryByRole('button', { name: 'Private Alice Sandbox' })).not.toBeInTheDocument();
    if (transition === 'sign-in epoch') {
      identity = alice();
      fireEvent.click(screen.getByRole('button', { name: 'Recheck test identity' }));
      await screen.findByRole('tab', { name: 'Sandboxes & usage' });
    }
    await act(async () =>
      resolveLate({ ...reportFixture(true), scope: { principal: 'realm:alice', admin: true } }),
    );
    await screen.findByText(
      transition === 'capability' ? 'Usage access unavailable' : 'Retrying usage automatically…',
    );
    expect(screen.queryByRole('button', { name: 'Private Alice Sandbox' })).not.toBeInTheDocument();
    expect(metrics.sandboxes).toHaveBeenCalledTimes(1);
  },
);

function CapacityProbe({ client }: { client: CmaWebClient }) {
  useSandboxCapacity(client, 'realm:alice');
  return null;
}

it('pauses protected navigation reads through an uncertain session and resumes once authenticated', async () => {
  window.history.replaceState(null, '', '/operations?tab=activity');
  const account = alice();
  if (account.status !== 'authenticated') throw new Error('Expected authenticated fixture');
  const authenticated: AuthState = {
    ...account,
    user: { ...account.user, capabilities: [...account.user.capabilities, 'configure_platform'] },
  };
  let resolveIdentity!: (next: AuthState) => void;
  const provider: AuthProvider = {
    current: vi.fn(
      () =>
        new Promise<AuthState>((resolve) => {
          resolveIdentity = resolve;
        }),
    ),
    signIn: vi.fn(async () => {}),
    signOut: vi.fn(async () => {}),
  };
  let hasCookie = false;
  let anonymousRequests = 0;
  const calls = new Map<string, number>();
  const paths = ['/api/v1/sandboxes', '/api/v1/sandboxes/capacity', '/api/v1/organizations'];
  const fetcher: typeof fetch = async (input, init) => {
    const url = new URL(String(input), window.location.origin);
    if (!paths.includes(url.pathname)) return new Response(null, { status: 503 });
    calls.set(url.pathname, (calls.get(url.pathname) ?? 0) + 1);
    if (!hasCookie || init?.credentials !== 'include') {
      anonymousRequests++;
      return Response.json({ code: 'not_authenticated' }, { status: 401 });
    }
    return Response.json(
      url.pathname.endsWith('/capacity')
        ? {
            resource: 'sandbox',
            limit: 2,
            live: 0,
            available: 2,
            counts_by_status: {},
            observed_at: new Date().toISOString(),
          }
        : { items: [], next: null },
    );
  };
  const client = createRestWebClient(createClient({ fetch: fetcher }));
  render(
    <AuthContextProvider provider={provider}>
      <CmaClientProvider client={client} operator={operatorClient()}>
        <RecheckIdentity />
        <CapacityProbe client={client} />
        <App />
      </CmaClientProvider>
    </AuthContextProvider>,
  );
  await waitFor(() => expect(resolveIdentity).toBeDefined());
  expect(calls.size).toBe(0);

  hasCookie = true;
  await act(async () => resolveIdentity(authenticated));
  const activity = await screen.findByRole('tab', { name: 'Activity' }, { timeout: 5000 });
  const operationsTabs = () => {
    fireEvent.click(screen.getByRole('tab', { name: 'Sandboxes & usage' }));
    fireEvent.click(screen.getByRole('tab', { name: 'Warm pools' }));
    fireEvent.click(activity);
  };
  await waitFor(() => expect(paths.map((path) => calls.get(path))).toEqual([1, 1, 1]));
  operationsTabs();
  expect(paths.map((path) => calls.get(path))).toEqual([1, 1, 1]);

  hasCookie = false;
  fireEvent(document, new Event('visibilitychange'));
  operationsTabs();
  await act(async () =>
    resolveIdentity({
      status: 'error',
      message: 'The identity provider is unavailable.',
      reason: 'identity_unavailable',
    }),
  );
  fireEvent(document, new Event('visibilitychange'));
  operationsTabs();
  expect(paths.map((path) => calls.get(path))).toEqual([1, 1, 1]);
  expect(anonymousRequests).toBe(0);

  // The visibility check above owns the pending session request.
  hasCookie = true;
  await act(async () => resolveIdentity(authenticated));
  await waitFor(() => expect(paths.map((path) => calls.get(path))).toEqual([2, 2, 2]));
  expect(anonymousRequests).toBe(0);
});
