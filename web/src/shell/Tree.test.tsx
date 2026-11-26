import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { navigationFixture } from '../test/hierarchy';
import type { TreeAction, TreePermissions } from './Tree';
import { renderTree } from './Tree.test-support';

const READ_ONLY: TreePermissions = {
  createWorkspace: false,
  createAgent: false,
  pauseSandbox: false,
  deleteSandbox: false,
  removeWorkspace: false,
  stopAgent: false,
  deleteAgent: false,
  exportHistory: false,
};

describe('Sandbox navigation tree', () => {
  it('locks only the Sandbox whose lifecycle request is still being confirmed', async () => {
    const actor = userEvent.setup();
    renderTree({
      busySandboxIds: new Set(['sbx_1']),
      items: [
        ...navigationFixture(),
        ...navigationFixture().map((item) => ({
          sandbox: { ...item.sandbox, id: 'sbx_2', name: 'Other sandbox', workspaces: [] },
          workspaces: [],
        })),
      ],
    });
    await actor.click(screen.getByRole('button', { name: 'Options for Atlas' }));
    expect(screen.getByRole('menuitem', { name: /Pause/ })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
    await actor.keyboard('{Escape}');
    await actor.click(screen.getByRole('button', { name: 'Options for Other sandbox' }));
    for (const name of ['Pause', 'New agent', 'Delete'])
      expect(screen.getByRole('menuitem', { name })).toHaveAttribute('aria-disabled', 'false');
  });

  it('allows removing a Workspace with live Agents through the deletion dialog', async () => {
    const onAction = vi.fn();
    renderTree({ onAction });
    const actor = userEvent.setup();
    await actor.click(screen.getByRole('button', { name: 'Options for /workspaces/chrono-bot' }));
    const remove = screen.getByRole('menuitem', { name: 'Remove' });
    expect(remove).toHaveAttribute('aria-disabled', 'false');
    await actor.click(remove);
    expect(onAction).toHaveBeenCalledExactlyOnceWith({
      kind: 'remove_workspace',
      workspaceId: 'wsp_1',
    });
  });

  it('groups live and paused Sandboxes separately and remembers each group per owner', async () => {
    const actor = userEvent.setup();
    const items = [
      ...navigationFixture(),
      ...navigationFixture('paused').map((item) => ({
        ...item,
        sandbox: { ...item.sandbox, id: 'sbx_paused', name: 'Sandbox Machine - paused' },
        workspaces: [],
      })),
    ];
    const view = renderTree({ items });
    expect(screen.getByRole('tree', { name: 'Live sandboxes' })).toHaveTextContent('Atlas');
    expect(screen.getByRole('tree', { name: 'Paused sandboxes' })).toHaveTextContent(
      'Sandbox Machine - paused',
    );
    await actor.click(screen.getByRole('button', { name: 'Collapse Live sandboxes' }));
    expect(screen.queryByText('Atlas')).not.toBeInTheDocument();
    expect(screen.getByText('Sandbox Machine - paused')).toBeInTheDocument();
    view.unmount();
    const restored = renderTree({ items });
    expect(screen.getByRole('button', { name: 'Expand Live sandboxes' })).toHaveAttribute(
      'aria-expanded',
      'false',
    );
    expect(screen.getByText('Sandbox Machine - paused')).toBeInTheDocument();
    restored.unmount();
    renderTree({ items, viewerId: 'another-owner' });
    expect(screen.getByText('Atlas')).toBeInTheDocument();
  });

  it.each(['preparing', 'removing', 'failed'] as const)(
    'does not offer Workspace expansion while %s',
    (status) => {
      renderTree({
        items: navigationFixture().map((item) => ({
          ...item,
          workspaces: item.workspaces.map((workspace) => ({ ...workspace, status })),
        })),
      });
      expect(screen.getByText('/workspaces/chrono-bot')).toBeInTheDocument();
      expect(screen.queryByText('Agent one')).not.toBeInTheDocument();
      expect(
        screen.queryByRole('button', {
          name: /Expand \/workspaces\/chrono-bot|Collapse \/workspaces\/chrono-bot/,
        }),
      ).not.toBeInTheDocument();
      expect(
        screen.getByText('/workspaces/chrono-bot').closest('[role="treeitem"]'),
      ).not.toHaveAttribute('aria-expanded');
    },
  );

  it('moves a resuming Paused record into Changing while keeping retained conversations accessible', () => {
    const items = navigationFixture('paused').map((item) => ({
      ...item,
      sandbox: {
        ...item.sandbox,
        lifecycle: {
          ...item.sandbox.lifecycle!,
          operation: {
            purpose: 'revive' as const,
            phase: 'provision',
            startedAt: '2026-09-08T00:00:00Z',
          },
        },
      },
    }));
    renderTree({ items });
    expect(screen.getByRole('tree', { name: 'Changing sandboxes' })).toHaveTextContent('Atlas');
    expect(screen.getByRole('tree', { name: 'Paused sandboxes' })).not.toHaveTextContent('Atlas');
    expect(screen.getByRole('link', { name: /Atlas/ })).toHaveTextContent('Resuming');
    expect(screen.getByRole('button', { name: 'Collapse Atlas' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /Agent one/ })).toHaveTextContent('Resuming');
  });

  it.each(['capture', 'resume_writers'] as const)(
    'keeps Live during %s and disables Pause without restricting child creation',
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
      expect(screen.getByRole('link', { name: /Atlas/ })).toHaveTextContent('Live');
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
      await actor.click(screen.getByRole('button', { name: 'Options for /workspaces/chrono-bot' }));
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
      expect(screen.getByText('Agent one')).toBeInTheDocument();
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

  it('keeps Agent navigation available during runtime updates', () => {
    renderTree({ items: navigationFixture('rotating') });
    expect(screen.getByText('Agent one')).toBeInTheDocument();
    expect(screen.getByText('/workspaces/chrono-bot')).toBeInTheDocument();
    expect(screen.getAllByText('Updating runtime').length).toBeGreaterThan(0);
  });

  it.each(['provisioning'] as const)(
    'hides child navigation for a %s Sandbox but keeps its status page accessible',
    async (status) => {
      const onNavigate = vi.fn();
      renderTree({ items: navigationFixture(status), onNavigate });
      expect(screen.getAllByRole('treeitem')).toHaveLength(1);
      expect(screen.queryByText('Agent one')).not.toBeInTheDocument();
      expect(screen.queryByText('/workspaces/chrono-bot')).not.toBeInTheDocument();
      expect(
        screen.queryByRole('button', {
          name: /Expand Atlas|Collapse Atlas/,
        }),
      ).not.toBeInTheDocument();
      await userEvent.setup().click(screen.getByRole('link', { name: /Atlas/ }));
      expect(onNavigate).toHaveBeenCalledWith({ kind: 'sandbox', sandboxId: 'sbx_1' });
    },
  );

  it.each(['active', 'retiring'] as const)(
    'keeps accepted deletion progress reachable outside active navigation while the projection is %s',
    async (status) => {
      const onViewDeletion = vi.fn();
      renderTree({
        items: navigationFixture(status),
        deletingSandboxIds: new Set(['sbx_1']),
        onViewDeletion,
      });
      expect(screen.queryByRole('treeitem')).not.toBeInTheDocument();
      expect(screen.queryByText('Agent one')).not.toBeInTheDocument();
      await userEvent.setup().click(screen.getByText('Cleaning up 1 sandboxes'));
      await userEvent.setup().click(screen.getByRole('button', { name: 'View status for Atlas' }));
      expect(
        screen.getByText(status === 'retiring' ? 'Deleting' : 'Deletion requested'),
      ).toBeVisible();
      expect(onViewDeletion).toHaveBeenCalledExactlyOnceWith('sbx_1');
    },
  );

  it('copies the real identifier from deletion progress without issuing a lifecycle command', async () => {
    const user = userEvent.setup();
    const write = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue();
    const onAction = vi.fn();
    const onViewDeletion = vi.fn();
    renderTree({ items: navigationFixture('retiring'), onAction, onViewDeletion });
    await user.click(screen.getByText('Cleaning up 1 sandboxes'));
    await user.click(screen.getByRole('button', { name: 'Copy identifier for Atlas' }));
    expect(write).toHaveBeenCalledExactlyOnceWith('sbx_1');
    expect(screen.getByRole('status')).toHaveTextContent('Identifier copied.');
    expect(onAction).not.toHaveBeenCalled();
    expect(onViewDeletion).not.toHaveBeenCalled();
    write.mockRestore();
  });

  it('falls back to the Sandbox page when no deletion dialog handler is available', async () => {
    const onNavigate = vi.fn();
    const onAction = vi.fn();
    renderTree({ items: navigationFixture('retiring'), onNavigate, onAction });
    const user = userEvent.setup();
    await user.click(screen.getByText('Cleaning up 1 sandboxes'));
    await user.click(screen.getByRole('button', { name: 'View status for Atlas' }));
    expect(onNavigate).toHaveBeenCalledExactlyOnceWith({ kind: 'sandbox', sandboxId: 'sbx_1' });
    expect(onAction).not.toHaveBeenCalled();
  });

  it('keeps history navigation without showing Ready while an Active Sandbox finishes revival', () => {
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
    expect(screen.getAllByRole('treeitem')).toHaveLength(3);
    expect(screen.getByRole('link', { name: /Agent one/ })).toHaveTextContent('Resuming');
    expect(screen.queryByText('Ready')).not.toBeInTheDocument();
  });

  it.each(['paused', 'pausing', 'reviving'] as const)(
    'keeps %s Agent history reachable while holding child mutations',
    async (status) => {
      const onNavigate = vi.fn();
      const onAction = vi.fn();
      renderTree({ items: navigationFixture(status), onNavigate, onAction });
      const actor = userEvent.setup();
      await actor.click(screen.getByRole('link', { name: /Agent one/ }));
      expect(onNavigate).toHaveBeenCalledExactlyOnceWith({ kind: 'agent', agentId: 'agt_1' });
      await actor.click(screen.getByRole('button', { name: 'Options for /workspaces/chrono-bot' }));
      for (const name of [/^New agent/, /^Remove/]) {
        const action = screen.getByRole('menuitem', { name });
        expect(action).toHaveAttribute('aria-disabled', 'true');
        await actor.click(action);
      }
      await actor.keyboard('{Escape}');
      await actor.click(screen.getByRole('button', { name: 'Options for Agent one' }));
      for (const name of [/^Stop/, /^Delete/]) {
        const action = screen.getByRole('menuitem', { name });
        expect(action).toHaveAttribute('aria-disabled', 'true');
        await actor.click(action);
      }
      expect(
        screen.getByRole('menuitem', { name: 'Export retained conversation' }),
      ).toHaveAttribute('aria-disabled', 'false');
      expect(onAction).not.toHaveBeenCalled();
    },
  );

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
    expect(screen.queryByText('/workspaces/chrono-bot')).not.toBeInTheDocument();
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

    await actor.click(screen.getByRole('button', { name: 'Options for /workspaces/chrono-bot' }));
    await actor.click(screen.getByRole('menuitem', { name: 'New agent' }));
    expect(actions.at(-1)).toEqual({ kind: 'new_agent', workspaceId: 'wsp_1' });

    await actor.click(screen.getByRole('button', { name: 'Options for Atlas' }));
    await actor.click(screen.getByRole('menuitem', { name: 'New agent' }));
    expect(actions.at(-1)).toEqual({ kind: 'new_agent', sandboxId: 'sbx_1' });

    await actor.click(screen.getByRole('button', { name: 'Options for Agent one' }));
    await actor.click(screen.getByRole('menuitem', { name: 'Stop' }));
    expect(actions.at(-1)).toEqual({ kind: 'stop_agent', agentId: 'agt_1' });
  });

  it('keeps New agent on a Workspace that is not ready listed with its reason', async () => {
    const actions: TreeAction[] = [];
    const actor = userEvent.setup();
    renderTree({
      items: navigationFixture().map((item) => ({
        ...item,
        workspaces: item.workspaces.map((workspace) => ({
          ...workspace,
          status: 'preparing' as const,
        })),
      })),
      onAction: (action) => actions.push(action),
    });
    await actor.click(screen.getByRole('button', { name: 'Options for /workspaces/chrono-bot' }));
    const create = screen.getByRole('menuitem', { name: /^New agent/ });
    expect(create).toHaveAttribute('aria-disabled', 'true');
    expect(create).toHaveTextContent('The Workspace is not ready.');
    await actor.click(create);
    expect(actions).toEqual([]);
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
