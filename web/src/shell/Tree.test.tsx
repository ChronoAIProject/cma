import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { navigationFixture } from '../test/hierarchy';
import type { NavigationSandbox } from './navigation';
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
    readonly items?: readonly NavigationSandbox[];
    readonly refreshError?: string;
    readonly lifecycleBusy?: boolean;
    readonly onAction?: (action: TreeAction) => void;
    readonly onNavigate?: ReturnType<typeof vi.fn>;
  } = {},
) {
  return render(
    <Tree
      viewerId={options.viewerId ?? 'viewer-one'}
      items={options.items ?? navigationFixture(options.paused === true ? 'paused' : 'active')}
      refreshError={options.refreshError}
      lifecycleBusy={options.lifecycleBusy}
      route={{ kind: 'home' }}
      permissions={options.permissions ?? FULL_ACCESS}
      onNavigate={options.onNavigate ?? vi.fn()}
      onAction={options.onAction ?? vi.fn()}
    />,
  );
}

describe('Sandbox navigation tree', () => {
  it.each(['capture', 'resume_writers'] as const)(
    'shows actual %s state and disables Pause without restricting child creation',
    async (phase) => {
      const onAction = vi.fn();
      const actor = userEvent.setup();
      const items = navigationFixture().map((item) => ({
        ...item,
        sandbox: {
          ...item.sandbox,
          lifecycle: {
            pause: { allowed: false, reason: 'checkpoint_in_progress' as const },
            revive: { allowed: false, reason: 'not_paused' as const },
            checkpoint: { phase, startedAt: '2026-09-08T00:00:00Z', epoch: 7 },
          },
        },
      }));
      renderTree({ items, onAction });
      expect(
        screen.getByText(phase === 'capture' ? 'Saving progress' : 'Finishing save'),
      ).toBeInTheDocument();
      await actor.click(screen.getByRole('button', { name: 'Options for Atlas' }));
      const pause = screen.getByRole('menuitem', { name: /^Pause/ });
      expect(pause).toHaveAttribute('aria-disabled', 'true');
      expect(pause).toHaveTextContent('Progress is being saved.');
      await actor.click(pause);
      expect(onAction).not.toHaveBeenCalled();
      expect(screen.getByRole('menuitem', { name: 'New workspace' })).toHaveAttribute(
        'aria-disabled',
        'false',
      );
      await actor.click(screen.getByRole('menuitem', { name: 'New workspace' }));
      await actor.click(screen.getByRole('button', { name: 'Options for chrono-bot' }));
      expect(screen.getByRole('menuitem', { name: 'New agent' })).toHaveAttribute(
        'aria-disabled',
        'false',
      );
    },
  );

  it.each(['active', 'paused'] as const)(
    'keeps %s eligibility unavailable after a failed refresh',
    async (status) => {
      renderTree({
        items: navigationFixture(status),
        refreshError: 'Sandbox status could not be refreshed. Reconnecting.',
      });
      expect(screen.getByRole('alert')).toHaveTextContent('Reconnecting');
      if (status === 'active') expect(screen.getByText('Agent one')).toBeInTheDocument();
      else expect(screen.queryByText('Agent one')).not.toBeInTheDocument();
      await userEvent.setup().click(screen.getByRole('button', { name: 'Options for Atlas' }));
      expect(
        screen.getByRole('menuitem', { name: status === 'paused' ? /^Resume/ : /^Pause/ }),
      ).toHaveAttribute('aria-disabled', 'true');
    },
  );

  it('uses the reported Resume eligibility and blocks duplicate pending lifecycle actions', async () => {
    const actor = userEvent.setup();
    const view = renderTree({ paused: true });
    await actor.click(screen.getByRole('button', { name: 'Options for Atlas' }));
    expect(screen.getByRole('menuitem', { name: 'Resume' })).toHaveAttribute(
      'aria-disabled',
      'false',
    );
    view.unmount();
    renderTree({ paused: true, lifecycleBusy: true });
    await actor.click(screen.getByRole('button', { name: 'Options for Atlas' }));
    expect(screen.getByRole('menuitem', { name: /^Resume/ })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
  });

  it('does not fall back to status when the server omits eligibility', async () => {
    renderTree({
      items: navigationFixture().map((item) => ({
        ...item,
        sandbox: { ...item.sandbox, lifecycle: undefined },
      })),
    });
    await userEvent.setup().click(screen.getByRole('button', { name: 'Options for Atlas' }));
    expect(screen.getByRole('menuitem', { name: /^Pause/ })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
    expect(screen.getByText('Action availability is not reported.')).toBeInTheDocument();
  });

  it('renders Sandbox, Workspace, and Agent rows without a Worktree navigation node', () => {
    renderTree();
    expect(screen.getByText('Atlas')).toBeInTheDocument();
    expect(screen.getByText('chrono-bot')).toBeInTheDocument();
    const agent = screen.getByText('Agent one').closest('[role="treeitem"]');
    expect(agent).not.toHaveAttribute('data-dimmed');
    expect(within(agent as HTMLElement).getByText('Done')).toBeInTheDocument();
    expect(screen.queryByText('agent/one')).not.toBeInTheDocument();
  });

  it.each(['paused', 'reviving', 'pausing', 'rotating', 'provisioning', 'retiring'] as const)(
    'hides child navigation for a %s Sandbox but keeps its status page accessible',
    async (status) => {
      const onNavigate = vi.fn();
      renderTree({ items: navigationFixture(status), onNavigate });
      expect(screen.getAllByRole('treeitem')).toHaveLength(1);
      expect(screen.queryByText('Agent one')).not.toBeInTheDocument();
      expect(screen.queryByText('chrono-bot')).not.toBeInTheDocument();
      expect(
        screen.queryByRole('button', { name: /Expand Atlas|Collapse Atlas/ }),
      ).not.toBeInTheDocument();
      await userEvent.setup().click(screen.getByRole('link', { name: /Atlas/ }));
      expect(onNavigate).toHaveBeenCalledWith({ kind: 'sandbox', sandboxId: 'sbx_1' });
    },
  );

  it('does not bring children back while an Active Sandbox is still finishing revival', () => {
    const items = navigationFixture().map((item) => ({
      ...item,
      sandbox: {
        ...item.sandbox,
        lifecycle: {
          ...item.sandbox.lifecycle!,
          operation: {
            purpose: 'revive' as const,
            phase: 'resume_agents',
            startedAt: '2026-09-08T00:00:00Z',
          },
        },
      },
    }));
    renderTree({ items });
    expect(screen.getAllByRole('treeitem')).toHaveLength(1);
  });

  it('removes terminal Sandboxes, Workspaces and Agents from working navigation', () => {
    const retired = renderTree({ items: navigationFixture('retired') });
    expect(screen.queryByText('Atlas')).not.toBeInTheDocument();
    expect(screen.getByText('No Sandboxes')).toBeInTheDocument();
    retired.unmount();
    const removed = renderTree({
      items: navigationFixture().map((item) => ({
        ...item,
        workspaces: item.workspaces.map((workspace) => ({
          ...workspace,
          status: 'removed' as const,
        })),
      })),
    });
    expect(screen.queryByText('chrono-bot')).not.toBeInTheDocument();
    removed.unmount();
    renderTree({ items: navigationFixture('active', 'deleted') });
    expect(screen.queryByText('Agent one')).not.toBeInTheDocument();
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
