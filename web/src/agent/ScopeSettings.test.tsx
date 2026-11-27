import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { AGENT_DETAIL, agentClient } from '../test/agent';
import { referenceClient } from '../test/references';
import { connectionClient } from '../connections/client';
import { accessRequests, AgentAccessError, type AgentAccessRequest } from './accessRequests';
import { ScopeSettings } from './ScopeSettings';

const baseline = ['chrono-llm-public', 'ornn-api'];
const request: AgentAccessRequest = {
  id: 'access-scope',
  agent_id: 'agt_1',
  status: 'applying',
  services: baseline,
  reason: 'Owner revised scope.',
};
beforeEach(() => {
  sessionStorage.clear();
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'owner',
    ready: true,
    checked_at: '2026-10-15T00:00:00Z',
    services: [],
  });
  vi.spyOn(accessRequests, 'list').mockResolvedValue([request]);
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});
function show(owner = 'owner') {
  return render(
    <CmaClientProvider client={agentClient()} profiles={referenceClient()}>
      <ScopeSettings owner={owner} detail={{ ...AGENT_DETAIL, reach: [...baseline, 'storage'] }} />
    </CmaClientProvider>,
  );
}
it('revokes optional access with exact scope and a stale-selection guard while the Sandbox stays active', async () => {
  const replace = vi.spyOn(accessRequests, 'replace').mockResolvedValue(request);
  show();
  for (const scope of baseline) {
    expect(screen.getByRole('checkbox', { name: `${scope} Required` })).toBeChecked();
    expect(screen.getByRole('checkbox', { name: `${scope} Required` })).toBeDisabled();
  }
  await userEvent.click(await screen.findByRole('checkbox', { name: 'Chrono Storage' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Save scope' })).toBeEnabled());
  await userEvent.click(screen.getByRole('button', { name: 'Save scope' }));
  await screen.findByText('Updating the key and notifying your Agent…');
  expect(replace).toHaveBeenCalledWith(
    'agt_1',
    baseline,
    [...baseline, 'storage'],
    expect.any(String),
    expect.any(AbortSignal),
  );
  expect(screen.queryByText(/Pause this Sandbox/)).not.toBeInTheDocument();
  expect(
    screen.queryByText('Agent key updated. Your Agent has been notified.'),
  ).not.toBeInTheDocument();
});
it('retains exact replacement across a lost reply and remount', async () => {
  const replace = vi
    .spyOn(accessRequests, 'replace')
    .mockRejectedValueOnce(new Error('Lost update response'))
    .mockResolvedValue(request);
  const first = show();
  await userEvent.click(await screen.findByRole('checkbox', { name: 'Chrono Storage' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Save scope' })).toBeEnabled());
  await userEvent.click(screen.getByRole('button', { name: 'Save scope' }));
  await screen.findByText('Lost update response');
  first.unmount();
  show();
  expect(await screen.findByRole('checkbox', { name: 'Chrono Storage' })).toBeDisabled();
  await userEvent.click(screen.getByRole('button', { name: 'Retry scope update' }));
  await waitFor(() => expect(replace).toHaveBeenCalledTimes(2));
  expect(replace.mock.calls[1]?.slice(0, 4)).toEqual(replace.mock.calls[0]?.slice(0, 4));
});
it('only confirms completion after the durable callback succeeds', async () => {
  vi.spyOn(accessRequests, 'replace').mockResolvedValue(request);
  show();
  await userEvent.click(await screen.findByRole('checkbox', { name: 'Chrono Storage' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Save scope' })).toBeEnabled());
  await userEvent.click(screen.getByRole('button', { name: 'Save scope' }));
  await screen.findByText('Updating the key and notifying your Agent…');
  vi.mocked(accessRequests.list).mockResolvedValue([{ ...request, status: 'succeeded' }]);
  await screen.findByText('Agent key updated. Your Agent has been notified.');
  expect(sessionStorage.length).toBe(0);
});
it('completes external connections before enabling final Save', async () => {
  const replace = vi.spyOn(accessRequests, 'replace');
  vi.mocked(connectionClient.check).mockResolvedValue({
    owner: 'owner',
    ready: false,
    checked_at: '2026-10-15T00:00:00Z',
    services: [],
  });
  show();
  await userEvent.click(await screen.findByRole('checkbox', { name: 'Chrono Storage' }));
  await screen.findByRole('button', { name: 'Connect services before saving' });
  expect(screen.getByRole('button', { name: 'Save scope' })).toBeDisabled();
  expect(replace).not.toHaveBeenCalled();
  expect(connectionClient.check).toHaveBeenCalledWith(
    { type: 'agent_service_access', id: 'agt_1', services: baseline },
    undefined,
    expect.any(AbortSignal),
  );
});

it.each(['manual check', 'returning to the page'])(
  'enables Save after the selected service becomes connected by %s',
  async (refresh) => {
    const replace = vi.spyOn(accessRequests, 'replace').mockResolvedValue(request);
    const service = {
      service: 'github',
      display_name: 'GitHub',
      kind: 'nyxid',
      connection_mode: 'hosted',
    } as const;
    const report = { owner: 'owner', checked_at: '2026-10-15T00:00:00Z' };
    vi.mocked(connectionClient.check)
      .mockResolvedValueOnce({
        ...report,
        ready: false,
        services: [{ ...service, state: 'missing' }],
      })
      .mockResolvedValue({
        ...report,
        ready: true,
        services: [{ ...service, state: 'satisfied' }],
      });
    show();
    const github = await screen.findByRole('checkbox', { name: 'GitHub' });
    await userEvent.click(github);
    expect(await screen.findByText('Verify connections for GitHub before saving.')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Save scope' })).toBeDisabled();
    expect(replace).not.toHaveBeenCalled();

    if (refresh === 'returning to the page') fireEvent.focus(window);
    else await userEvent.click(screen.getByRole('button', { name: 'Check connections again' }));
    const save = screen.getByRole('button', { name: 'Save scope' });
    await waitFor(() => expect(save).toBeEnabled());
    expect(github).toBeChecked();
    expect(replace).not.toHaveBeenCalled();
    await userEvent.click(save);
    expect(replace).toHaveBeenCalledExactlyOnceWith(
      'agt_1',
      [...baseline, 'storage', 'github'],
      [...baseline, 'storage'],
      expect.any(String),
      expect.any(AbortSignal),
    );
  },
);
it('does not use a connection report for a different account', async () => {
  vi.mocked(connectionClient.check).mockResolvedValue({
    owner: 'other',
    ready: true,
    checked_at: '2026-10-15T00:00:00Z',
    services: [],
  });
  show();
  await userEvent.click(await screen.findByRole('checkbox', { name: 'Chrono Storage' }));
  await screen.findByText('Your CMA account changed. Open the Agent again.');
  expect(screen.getByRole('button', { name: 'Save scope' })).toBeDisabled();
});
it('aborts uncertain submission when leaving and retains identity for recovery', async () => {
  const replace = vi
    .spyOn(accessRequests, 'replace')
    .mockImplementation(() => new Promise(() => {}));
  const view = show();
  await userEvent.click(await screen.findByRole('checkbox', { name: 'Chrono Storage' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Save scope' })).toBeEnabled());
  await userEvent.click(screen.getByRole('button', { name: 'Save scope' }));
  const signal = replace.mock.calls[0]?.[4];
  view.unmount();
  expect(signal?.aborted).toBe(true);
  expect(sessionStorage.length).toBe(1);
});
it('rejects an obsolete connection reply after selection changes', async () => {
  let finish!: (report: Awaited<ReturnType<typeof connectionClient.check>>) => void;
  vi.mocked(connectionClient.check)
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    )
    .mockImplementation(() => new Promise(() => {}));
  show();
  const checkbox = await screen.findByRole('checkbox', { name: 'Chrono Storage' });
  await userEvent.click(checkbox);
  await userEvent.click(checkbox);
  await act(async () =>
    finish({ owner: 'owner', ready: true, checked_at: '2026-10-15T00:00:00Z', services: [] }),
  );
  expect(screen.getByRole('button', { name: 'Save scope' })).toBeDisabled();
});

it('reloads a stale scope only after a definite conflict, never after an uncertain failure', async () => {
  const replace = vi
    .spyOn(accessRequests, 'replace')
    .mockRejectedValue(new AgentAccessError(409, 'Scope changed since you opened it.'));
  show();
  await userEvent.click(await screen.findByRole('checkbox', { name: 'Chrono Storage' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Save scope' })).toBeEnabled());
  await userEvent.click(screen.getByRole('button', { name: 'Save scope' }));
  await screen.findByText('Scope changed since you opened it.');
  await userEvent.click(screen.getByRole('button', { name: 'Reload current scope' }));
  await waitFor(() => expect(sessionStorage.length).toBe(0));
  expect(replace).toHaveBeenCalledOnce();
});

it.each([503, 403])(
  'does not claim a grant on HTTP %s and retains the exact original command',
  async (status) => {
    const replace = vi
      .spyOn(accessRequests, 'replace')
      .mockRejectedValueOnce(new AgentAccessError(status, 'Scope update was not confirmed.'))
      .mockResolvedValue(request);
    const first = show();
    await userEvent.click(await screen.findByRole('checkbox', { name: 'Chrono Storage' }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Save scope' })).toBeEnabled());
    await userEvent.click(screen.getByRole('button', { name: 'Save scope' }));
    await screen.findByText('Scope update was not confirmed.');
    expect(
      screen.queryByText('Agent key updated. Your Agent has been notified.'),
    ).not.toBeInTheDocument();
    first.unmount();
    show();
    expect(replace).toHaveBeenCalledOnce();
    await userEvent.click(screen.getByRole('button', { name: 'Retry scope update' }));
    await waitFor(() => expect(replace).toHaveBeenCalledTimes(2));
    expect(replace.mock.calls[1]?.slice(0, 4)).toEqual(replace.mock.calls[0]?.slice(0, 4));
  },
);
