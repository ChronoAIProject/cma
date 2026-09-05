import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { navigationFixture } from '../test/hierarchy';
import { Tree, activityLabel, type TreeAction, type TreePermissions } from './Tree';

const FULL_ACCESS: TreePermissions = {
  createWorkspace: true,
  createAgent: true,
  pauseSandbox: true,
  deleteSandbox: true,
  removeWorkspace: true,
  stopAgent: true,
  deleteAgent: true,
};

const READ_ONLY: TreePermissions = {
  createWorkspace: false,
  createAgent: false,
  pauseSandbox: false,
  deleteSandbox: false,
  removeWorkspace: false,
  stopAgent: false,
  deleteAgent: false,
};

function renderTree(
  options: {
    readonly viewerId?: string;
    readonly permissions?: TreePermissions;
    readonly paused?: boolean;
    readonly onAction?: (action: TreeAction) => void;
    readonly onNavigate?: ReturnType<typeof vi.fn>;
  } = {},
) {
  return render(
    <Tree
      viewerId={options.viewerId ?? 'viewer-one'}
      items={navigationFixture(options.paused === true ? 'paused' : 'active')}
      route={{ kind: 'home' }}
      permissions={options.permissions ?? FULL_ACCESS}
      onNavigate={options.onNavigate ?? vi.fn()}
      onAction={options.onAction ?? vi.fn()}
    />,
  );
}

describe('Sandbox navigation tree', () => {
  it('renders Sandbox, Workspace, and Agent rows without a Worktree navigation node', () => {
    renderTree({ paused: true });
    expect(screen.getByText('Atlas')).toBeInTheDocument();
    expect(screen.getByText('chrono-bot')).toBeInTheDocument();
    const agent = screen.getByText('Agent one').closest('[role="treeitem"]');
    expect(agent).toHaveAttribute('data-dimmed');
    expect(within(agent as HTMLElement).getByText('Paused')).toBeInTheDocument();
    expect(screen.getByRole('img', { name: 'Sandbox paused' })).toBeInTheDocument();
    expect(screen.queryByText('agent/one')).not.toBeInTheDocument();
  });

  it('persists collapsed groups independently for each viewer', async () => {
    const actor = userEvent.setup();
    const first = renderTree({ viewerId: 'viewer-one' });
    await actor.click(screen.getByRole('button', { name: 'Collapse Atlas' }));
    expect(screen.queryByText('Agent one')).not.toBeInTheDocument();
    first.unmount();

    const restored = renderTree({ viewerId: 'viewer-one' });
    expect(screen.queryByText('Agent one')).not.toBeInTheDocument();
    restored.unmount();

    renderTree({ viewerId: 'viewer-two' });
    expect(screen.getByText('Agent one')).toBeInTheDocument();
  });

  it('offers the specified row actions and reports the chosen resource', async () => {
    const actions: TreeAction[] = [];
    const actor = userEvent.setup();
    renderTree({ onAction: (action) => actions.push(action) });

    await actor.click(screen.getByRole('button', { name: 'Options for Atlas' }));
    expect(screen.getByRole('menuitem', { name: 'Pause' })).toBeInTheDocument();
    expect(screen.getByRole('menuitem', { name: 'Delete' })).toBeInTheDocument();
    await actor.click(screen.getByRole('menuitem', { name: 'New workspace' }));
    expect(actions).toEqual([{ kind: 'new_workspace', sandboxId: 'sbx_1' }]);

    await actor.click(screen.getByRole('button', { name: 'Options for chrono-bot' }));
    await actor.click(screen.getByRole('menuitem', { name: 'New agent' }));
    expect(actions.at(-1)).toEqual({ kind: 'new_agent', workspaceId: 'wsp_1' });

    await actor.click(screen.getByRole('button', { name: 'Options for Agent one' }));
    await actor.click(screen.getByRole('menuitem', { name: 'Stop' }));
    expect(actions.at(-1)).toEqual({ kind: 'stop_agent', agentId: 'agt_1' });
  });

  it('removes creation and control affordances from a visitor tree', () => {
    renderTree({ permissions: READ_ONLY });
    expect(screen.queryByRole('button', { name: /options for/i })).not.toBeInTheDocument();
    expect(screen.queryByText('New workspace')).not.toBeInTheDocument();
    expect(screen.getByText('Agent one')).toBeInTheDocument();
  });

  it('moves through visible rows with tree keyboard controls and activates links', async () => {
    const onNavigate = vi.fn();
    const actor = userEvent.setup();
    renderTree({ onNavigate });
    const rows = screen.getAllByRole('treeitem');
    rows[0]?.focus();
    await actor.keyboard('{ArrowDown}');
    expect(rows[1]).toHaveFocus();
    await actor.keyboard('{ArrowDown}{Enter}');
    expect(onNavigate).toHaveBeenCalledWith({ kind: 'agent', agentId: 'agt_1' });
  });
});

describe('activity labels', () => {
  it('formats recent activity without unstable locale output', () => {
    const now = Date.parse('2026-09-06T01:05:00Z');
    expect(activityLabel('2026-09-06T01:04:45Z', now)).toBe('Now');
    expect(activityLabel('2026-09-06T01:00:00Z', now)).toBe('5m');
    expect(activityLabel('2026-09-05T23:05:00Z', now)).toBe('2h');
    expect(activityLabel('not-a-date', now)).toBe('Unknown activity');
  });
});
