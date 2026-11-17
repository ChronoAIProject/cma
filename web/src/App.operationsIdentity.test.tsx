import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { App } from './App';
import { CmaClientProvider } from './api/context';
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
