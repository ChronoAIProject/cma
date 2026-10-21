import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { ApiError } from '@chronoai/cma-sdk';
import { afterEach, expect, it, vi } from 'vitest';
import type { CmaWebClient } from '../api/client';
import type { TreePermissions } from '../shell/Tree';
import { agentClient } from '../test/agent';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { OwnerWorkspacePage } from './OwnerWorkspacePage';

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
  vi.useRealTimers();
});

function show(client: CmaWebClient = agentClient(), onAction = vi.fn(), onNavigate = vi.fn()) {
  return render(
    <OwnerWorkspacePage
      client={client}
      workspaceId="wsp_1"
      revision={0}
      permissions={permissions}
      busy={false}
      onAction={onAction}
      onNavigate={onNavigate}
    />,
  );
}

function openMore() {
  fireEvent.click(screen.getByRole('button', { name: 'More actions' }));
}

it('places the Workspace in its Sandbox and states its facts', async () => {
  const onNavigate = vi.fn();
  show(agentClient(), vi.fn(), onNavigate);
  expect(await screen.findByRole('heading', { level: 1, name: 'chrono-bot' })).toBeInTheDocument();
  const crumbs = within(screen.getByRole('navigation', { name: 'Breadcrumb' }));
  const sandbox = crumbs.getByRole('link', { name: 'Atlas' });
  expect(sandbox).toHaveAttribute('href', '/sandboxes/sbx_1');
  fireEvent.click(sandbox);
  expect(onNavigate).toHaveBeenCalledWith({ kind: 'sandbox', sandboxId: 'sbx_1' });
  expect(crumbs.getByText('chrono-bot')).toHaveAttribute('aria-current', 'page');
  expect(screen.getByText('Ready', { selector: '.page-header__badge .badge__word' })).toBeVisible();
  const fact = (label: string) => screen.getByText(label).parentElement?.querySelector('dd');
  expect(fact('Path')).toHaveTextContent('/workspaces/chrono-bot');
  expect(within(fact('Source')!).getByRole('link')).toHaveAttribute(
    'href',
    'https://example.test/chrono-bot.git',
  );
  expect(fact('Branch or ref')).toHaveTextContent('Default branch');
  expect(fact('Resolved commit')).toHaveTextContent('abc123');
  expect(document.title).toBe('chrono-bot · CMA');
});

it('pairs each Agent with its Worktree', async () => {
  show();
  const table = await screen.findByRole('table');
  const [, row] = within(table).getAllByRole('row');
  expect(within(row!).getByRole('link', { name: 'Agent one' })).toHaveAttribute(
    'href',
    '/agents/agt_1',
  );
  expect(within(row!).getByText('agent/one')).toHaveAttribute('title', 'agent-one');
  expect(within(row!).getAllByText('Ready')).toHaveLength(2);
});

it('starts an Agent in this Workspace and deletes it from More actions', async () => {
  const onAction = vi.fn();
  show(agentClient(), onAction);
  fireEvent.click(await screen.findByRole('button', { name: 'New agent in this Workspace' }));
  openMore();
  fireEvent.click(screen.getByRole('menuitem', { name: 'Delete workspace' }));
  expect(onAction.mock.calls).toEqual([
    [{ kind: 'new_agent', workspaceId: 'wsp_1' }],
    [{ kind: 'remove_workspace', workspaceId: 'wsp_1' }],
  ]);
});

it('holds owner actions with their reasons while the Sandbox is paused', async () => {
  const onAction = vi.fn();
  show(agentClient({ readSandbox: async () => sandboxFixture('paused') }), onAction);
  const create = await screen.findByRole('button', { name: 'New agent in this Workspace' });
  expect(create).toBeDisabled();
  expect(create).toHaveAccessibleDescription(
    'Agents and Workspaces can start when the Sandbox is active.',
  );
  expect(screen.getByText(/This Sandbox is paused/)).toBeInTheDocument();
  expect(
    screen.getByText('Paused', { selector: '.page-header__badge .badge__word' }),
  ).toBeVisible();
  openMore();
  const remove = screen.getByRole('menuitem', { name: /^Delete workspace/ });
  expect(remove).toHaveAttribute('aria-disabled', 'true');
  expect(remove).toHaveTextContent('Agents and Workspaces can start when the Sandbox is active.');
  fireEvent.click(remove);
  expect(onAction).not.toHaveBeenCalled();
});

it('describes an empty-directory Workspace without inventing a source', async () => {
  show(
    agentClient({
      readWorkspace: async () => ({
        ...workspaceFixture(),
        source: { type: 'empty' },
        resolvedCommit: undefined,
        agents: [],
        worktrees: [],
      }),
    }),
  );
  await screen.findByRole('heading', { level: 1, name: 'chrono-bot' });
  expect(screen.getByText('Empty directory')).toBeInTheDocument();
  expect(screen.getByText('None')).toBeInTheDocument();
  expect(screen.getByText('Not reported')).toBeInTheDocument();
  expect(screen.getByText('No agents')).toBeInTheDocument();
});

it('never turns a non-web source address into a link', async () => {
  show(
    agentClient({
      readWorkspace: async () => ({
        ...workspaceFixture(),
        source: { type: 'git_repository', url: 'javascript:alert(1)', reference: 'main' },
      }),
    }),
  );
  await screen.findByRole('heading', { level: 1, name: 'chrono-bot' });
  expect(screen.getByText('javascript:alert(1)').closest('a')).toBeNull();
  expect(screen.getByText('main')).toBeInTheDocument();
});

it('reports a failed Workspace as an error with its remediation', async () => {
  show(
    agentClient({
      readWorkspace: async () => ({
        ...workspaceFixture(),
        status: 'failed',
        failure: { class: 'clone_failed', detail: 'internal', remediation: 'Check the URL.' },
      }),
    }),
  );
  const alert = await screen.findByRole('alert');
  expect(alert).toHaveTextContent('This Workspace needs attention.');
  expect(alert).toHaveTextContent('Check the URL.');
  expect(screen.queryByText('internal')).not.toBeInTheDocument();
});

it('keeps content through a transient failure and gives way only to a refusal', async () => {
  vi.useFakeTimers();
  const readWorkspace = vi
    .fn()
    .mockResolvedValueOnce(workspaceFixture())
    .mockRejectedValueOnce(new Error('offline'))
    .mockRejectedValueOnce(new ApiError(404, 'Workspace not found'));
  show(agentClient({ readWorkspace }));
  await act(async () => {});
  expect(screen.getByRole('heading', { level: 1, name: 'chrono-bot' })).toBeInTheDocument();
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(screen.getByRole('alert')).toHaveTextContent('Reconnecting');
  expect(screen.getByRole('heading', { level: 1, name: 'chrono-bot' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'New agent in this Workspace' })).toBeDisabled();
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(screen.getByRole('heading', { name: 'Workspace unavailable' })).toBeInTheDocument();
  expect(readWorkspace).toHaveBeenCalledTimes(3);
});

it('refuses a Workspace read that names another resource', async () => {
  show(agentClient({ readWorkspace: async () => ({ ...workspaceFixture(), id: 'wsp_other' }) }));
  expect(await screen.findByRole('heading', { name: 'Workspace unavailable' })).toBeInTheDocument();
  expect(screen.getByText('Workspace projection referred to another resource.')).toBeVisible();
});
