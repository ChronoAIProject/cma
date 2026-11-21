import { fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { SandboxDetail } from '../api/types';
import type { TreePermissions } from '../shell/Tree';
import { agentClient } from '../test/agent';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { SandboxPage } from './SandboxPage';

const permissions: TreePermissions = {
  createWorkspace: true,
  createAgent: true,
  pauseSandbox: true,
  deleteSandbox: true,
  removeWorkspace: true,
  stopAgent: true,
  deleteAgent: true,
  exportHistory: true,
};

afterEach(() => {
  window.history.replaceState(null, '', '/');
});

function page(
  readSandbox: () => Promise<SandboxDetail> = async () => sandboxFixture(),
  overrides: Parameters<typeof agentClient>[0] = {},
) {
  return render(
    <SandboxPage
      client={agentClient({ readSandbox, ...overrides })}
      sandboxId="sbx_1"
      revision={0}
      permissions={permissions}
      busy={false}
      onAction={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
}

it('opens the tab the URL names and keeps the selection in the query', async () => {
  window.history.replaceState(null, '', '/sandboxes/sbx_1?tab=access');
  page();
  expect(await screen.findByRole('tab', { name: 'Access' })).toHaveAttribute(
    'aria-selected',
    'true',
  );
  expect(screen.getByRole('heading', { name: 'Service access' })).toBeVisible();
  fireEvent.click(screen.getByRole('tab', { name: 'Workspaces' }));
  expect(window.location.search).toBe('?tab=workspaces');
  fireEvent.click(screen.getByRole('tab', { name: 'Overview' }));
  expect(window.location.pathname).toBe('/sandboxes/sbx_1');
  expect(window.location.search).toBe('');
});

it('opens Overview for an unknown tab and for a settled Sandbox without one', async () => {
  window.history.replaceState(null, '', '/sandboxes/sbx_1?tab=billing');
  page();
  expect(await screen.findByRole('tab', { name: 'Overview' })).toHaveAttribute(
    'aria-selected',
    'true',
  );
  expect(screen.getByRole('heading', { name: 'Environment' })).toBeVisible();
});

it('opens a Sandbox that is still being prepared on its Activity', async () => {
  page(async () => ({ ...sandboxFixture('provisioning'), workspaces: [] }));
  expect(await screen.findByRole('tab', { name: 'Activity' })).toHaveAttribute(
    'aria-selected',
    'true',
  );
  expect(screen.queryByRole('button', { name: 'View progress' })).not.toBeInTheDocument();
});

it('offers the recorded progress of a state from its alert', async () => {
  page(async () => sandboxFixture('paused'));
  const alert = await screen.findByText(/Your work is preserved/);
  fireEvent.click(screen.getByRole('button', { name: 'View progress' }));
  expect(screen.getByRole('tab', { name: 'Activity' })).toHaveAttribute('aria-selected', 'true');
  expect(alert).toBeVisible();
  expect(screen.queryByRole('button', { name: 'View progress' })).not.toBeInTheDocument();
});

it('reports a failure once, as an error with the recorded remediation', async () => {
  page(async () => ({
    ...sandboxFixture('failed'),
    failure: { class: 'install_failed', detail: 'Install failed.', remediation: 'Retry later.' },
  }));
  const alert = await screen.findByRole('alert');
  expect(alert).toHaveTextContent('Your sandbox needs attention.');
  expect(alert).toHaveTextContent('Retry later.');
  expect(screen.queryByText(/Review the recorded failure/)).not.toBeInTheDocument();
});

it('lists each Workspace with its path, Agents and a creation action of its own', async () => {
  page();
  fireEvent.click(await screen.findByRole('tab', { name: 'Workspaces' }));
  const table = await screen.findByRole('table');
  const [header, row] = within(table).getAllByRole('row');
  expect(
    within(header!)
      .getAllByRole('columnheader')
      .map((cell) => cell.textContent),
  ).toEqual(['Workspace', 'Path', 'Agents', 'Actions']);
  expect(within(row!).getByRole('link', { name: 'chrono-bot' })).toHaveAttribute(
    'href',
    '/workspaces/wsp_1',
  );
  expect(within(row!).getByText('/workspaces/chrono-bot')).toBeInTheDocument();
  expect(within(row!).getByRole('link', { name: 'Agent one' })).toHaveAttribute(
    'href',
    '/agents/agt_1',
  );
  expect(await within(row!).findByText('gpt-5')).toBeInTheDocument();
  expect(within(row!).getByRole('button', { name: 'New agent in chrono-bot' })).toBeEnabled();
});

it('holds New agent here for a Workspace that is not ready', async () => {
  page(
    async () => ({
      ...sandboxFixture(),
      workspaces: [{ ...workspaceFixture(), status: 'preparing' }],
    }),
    { readWorkspace: async () => ({ ...workspaceFixture(), status: 'preparing' }) },
  );
  fireEvent.click(await screen.findByRole('tab', { name: 'Workspaces' }));
  const button = await screen.findByRole('button', { name: 'New agent in chrono-bot' });
  expect(button).toBeDisabled();
  expect(button).toHaveAccessibleDescription('The Workspace is not ready.');
});
