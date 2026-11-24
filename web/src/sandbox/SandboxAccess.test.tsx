import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import type { AgentDetail } from '../api/types';
import { accessRequests } from '../agent/accessRequests';
import { saveScopeUpdate } from '../agent/scopeUpdateStorage';
import { connectionClient } from '../connections/client';
import type { SandboxContents } from '../resources/retainedOwnerViews';
import { AGENT_DETAIL, agentClient } from '../test/agent';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { referenceClient } from '../test/references';
import { SandboxAccess } from './SandboxAccess';

const baseline = ['chrono-llm-public', 'ornn-api'];
const second: AgentDetail = {
  ...AGENT_DETAIL,
  id: 'agt_2',
  worktree: { ...AGENT_DETAIL.worktree, id: 'wkt_2', agentId: 'agt_2' },
};

beforeEach(() => {
  sessionStorage.clear();
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'owner',
    ready: true,
    checked_at: '2026-10-21T00:00:00Z',
    services: [],
  });
  vi.spyOn(accessRequests, 'list').mockResolvedValue([]);
});
afterEach(() => {
  vi.restoreAllMocks();
});

function show(
  contents: SandboxContents | undefined,
  { notOwner = false, onRetry = vi.fn() }: { notOwner?: boolean; onRetry?: () => void } = {},
) {
  return render(
    <CmaClientProvider client={agentClient()} profiles={referenceClient()}>
      <SandboxAccess
        sandbox={{ ...sandboxFixture(), nyxidServices: [...baseline, 'github'] }}
        contents={contents}
        owner={notOwner ? undefined : 'owner'}
        active
        onRetry={onRetry}
      />
    </CmaClientProvider>,
  );
}

it('edits the Sandbox-wide scope with the required services locked and saves through an Agent', async () => {
  const replace = vi.spyOn(accessRequests, 'replace').mockResolvedValue({
    id: 'access-1',
    agent_id: 'agt_1',
    status: 'applying',
    services: baseline,
    reason: 'Owner revised scope.',
  });
  show({
    workspaces: [workspaceFixture()],
    agents: [{ ...AGENT_DETAIL, reach: [...baseline, 'storage'] }],
    failed: false,
  });
  expect(screen.getByRole('group', { name: 'Agent key scope' })).toBeVisible();
  expect(
    screen.getByText('These services are shared by all Agents in this Sandbox.'),
  ).toBeVisible();
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
});

it('matches the same Sandbox services with or without surrounding search whitespace', async () => {
  show({ workspaces: [workspaceFixture()], agents: [AGENT_DETAIL], failed: false });
  await screen.findByRole('checkbox', { name: 'GitHub' });

  const search = screen.getByRole('textbox', { name: 'Search services' });
  const services = within(screen.getByRole('list', { name: 'Services' }));
  for (const name of ['GitHub', 'Chrono Storage']) {
    for (const query of [name, ` ${name}`, `${name} `]) {
      fireEvent.change(search, { target: { value: query } });
      expect(services.getAllByRole('checkbox')).toEqual([services.getByRole('checkbox', { name })]);
    }
  }
  fireEvent.change(search, { target: { value: 'no matching service' } });
  expect(services.queryByRole('checkbox')).not.toBeInTheDocument();
});

it('continues a retained scope update through the Agent that holds it', async () => {
  saveScopeUpdate('owner', 'agt_2', {
    key: 'retained-key',
    services: baseline,
    expectedServices: [...baseline, 'github'],
  });
  show({ workspaces: [workspaceFixture()], agents: [AGENT_DETAIL, second], failed: false });
  expect(await screen.findByRole('button', { name: 'Retry scope update' })).toBeEnabled();
});

it('prefers a running Agent when none holds an update', async () => {
  const replace = vi.spyOn(accessRequests, 'replace').mockRejectedValue(new Error('Refused.'));
  show({
    workspaces: [workspaceFixture()],
    agents: [
      { ...AGENT_DETAIL, status: 'stopped', reach: [...baseline, 'storage'] },
      { ...second, reach: [...baseline, 'storage'] },
    ],
    failed: false,
  });
  await userEvent.click(await screen.findByRole('checkbox', { name: 'Chrono Storage' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Save scope' })).toBeEnabled());
  await userEvent.click(screen.getByRole('button', { name: 'Save scope' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('Refused.');
  expect(replace.mock.calls[0]?.[0]).toBe('agt_2');
});

it('lists effective access read-only and says how to enable changes without an Agent', () => {
  show({ workspaces: [{ ...workspaceFixture(), agents: [] }], agents: [], failed: false });
  expect(screen.queryByRole('group', { name: 'Agent key scope' })).not.toBeInTheDocument();
  expect(screen.getByText('NyxID services')).toBeInTheDocument();
  expect(
    screen.getByText('Service access can be changed once this Sandbox has an Agent.'),
  ).toBeInTheDocument();
});

it('offers a retry when the Agents could not be read', async () => {
  const onRetry = vi.fn();
  show({ workspaces: [workspaceFixture()], agents: [], failed: true }, { onRetry });
  expect(screen.getByText(/Service access could not be loaded/)).toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Try again' }));
  expect(onRetry).toHaveBeenCalledOnce();
});

it('waits quietly while Agents are still being read, and offers no editor to a non-owner', () => {
  const view = show({ workspaces: [workspaceFixture()], agents: [], failed: false });
  expect(screen.queryByText(/Service access can be changed/)).not.toBeInTheDocument();
  expect(screen.queryByText(/could not be loaded/)).not.toBeInTheDocument();
  view.unmount();
  show(
    { workspaces: [workspaceFixture()], agents: [AGENT_DETAIL], failed: false },
    { notOwner: true },
  );
  expect(screen.queryByRole('group', { name: 'Agent key scope' })).not.toBeInTheDocument();
  expect(screen.getByText('NyxID services')).toBeInTheDocument();
});
