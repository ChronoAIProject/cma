import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { AgentSummary, SandboxDetail, SandboxRuntimeUpdate } from '../api/types';
import { navigationFixture } from '../test/hierarchy';
import { AGENT_DETAIL } from '../test/agent';
import { Tree, type TreeProps } from './Tree';

const update: SandboxRuntimeUpdate = {
  currentVersion: 'runtime-before',
  latestVersion: 'runtime-after',
  updateAvailable: true,
  state: 'failed',
  canUpdate: false,
  schedule: { timezone: 'Asia/Singapore', localTime: '02:00' },
  preservesHome: true,
};

function items(
  agent: Partial<AgentSummary> = {},
  sandbox: Partial<SandboxDetail> = {},
): TreeProps['items'] {
  return navigationFixture('rotating').map((item) => ({
    sandbox: {
      ...item.sandbox,
      lifecycle: {
        ...item.sandbox.lifecycle!,
        operation: {
          purpose: 'image_rotation',
          phase: 'conversation_reopen',
          startedAt: '2026-09-06T00:10:00Z',
        },
      },
      runtimeUpdate: update,
      ...sandbox,
    },
    workspaces: item.workspaces.map((workspace) => ({
      ...workspace,
      agents: workspace.agents.map((original) => ({
        ...original,
        taskStatus: 'completed',
        ...agent,
      })),
    })),
  }));
}

function tree(overrides: Partial<TreeProps> = {}) {
  return (
    <Tree
      viewerId="rotation-viewer"
      view="agent"
      items={items()}
      route={{ kind: 'home' }}
      permissions={{
        createWorkspace: true,
        createAgent: true,
        pauseSandbox: true,
        deleteSandbox: true,
        removeWorkspace: true,
        stopAgent: true,
        deleteAgent: true,
        exportHistory: true,
      }}
      onNavigate={vi.fn()}
      onAction={vi.fn()}
      {...overrides}
    />
  );
}

afterEach(() => vi.useRealTimers());

describe('Agent navigation during environment updates', () => {
  it('retains the loaded outcome through active, rotating, and stale navigation observations', () => {
    const currentTask = { agent: AGENT_DETAIL, status: 'failed' as const };
    const activeItems = items(
      { status: 'working' },
      {
        status: 'active',
        lifecycle: navigationFixture()[0]!.sandbox.lifecycle,
      },
    );
    const view = render(tree({ items: activeItems, currentTask }));
    expect(screen.getByRole('treeitem')).toHaveAccessibleName(/Failed/);
    expect(screen.queryByRole('group', { name: 'Active' })).not.toBeInTheDocument();
    view.rerender(tree({ items: items({ status: 'working' }), currentTask }));
    expect(screen.getByRole('treeitem')).toHaveTextContent('Failed · Runtime update failed');
    expect(screen.getByRole('group', { name: 'Earlier' })).toBeInTheDocument();
    view.rerender(
      tree({
        items: items({ status: 'working' }),
        currentTask,
        refreshError: 'Refresh unavailable',
      }),
    );
    expect(screen.getByRole('treeitem')).toHaveTextContent('Status unavailable');
    expect(screen.queryByText('Runtime update failed')).not.toBeInTheDocument();
  });

  it.each(['ready', 'working'] as const)(
    'shows failed rotation separately from a %s Agent outcome',
    async (status) => {
      const onNavigate = vi.fn();
      const onAction = vi.fn();
      const { container } = render(tree({ items: items({ status }), onNavigate, onAction }));
      expect(screen.queryByRole('group', { name: 'Needs attention' })).not.toBeInTheDocument();
      expect(screen.getByText('1 update failed · Fix')).toBeVisible();
      const row = within(
        screen.getByRole('list', { name: 'Environments with failed updates' }),
      ).getByRole('treeitem');
      expect(row).toHaveTextContent('Runtime update failed');
      expect(row).toHaveAccessibleName(/Runtime update failed/);
      if (status === 'ready') expect(row).toHaveTextContent('Turn ended');
      else expect(row).not.toHaveTextContent('Turn ended');
      expect(screen.queryByRole('group', { name: 'Active' })).not.toBeInTheDocument();
      expect(screen.queryByText('Preparing')).not.toBeInTheDocument();
      expect(container.querySelector('.task-running')).toBeNull();
      expect(container.querySelector('[data-pose="preparing"]')).toBeNull();
      const actor = userEvent.setup();
      await actor.click(screen.getByRole('button', { name: 'Options for Agent one' }));
      expect(screen.getByRole('menuitem', { name: /^Delete(?: Available|$)/ })).toHaveAttribute(
        'aria-disabled',
        'true',
      );
      await actor.click(screen.getByRole('menuitem', { name: 'View environment' }));
      expect(onNavigate).toHaveBeenCalledExactlyOnceWith({ kind: 'sandbox', sandboxId: 'sbx_1' });
      expect(onAction).not.toHaveBeenCalled();
    },
  );

  it('filters parked updates by Needs attention and genuinely starting Agents by Preparing', async () => {
    const starting = navigationFixture('provisioning', 'starting').map((item) => ({
      ...item,
      workspaces: item.workspaces.map((workspace) => ({
        ...workspace,
        agents: workspace.agents.map((agent) => ({
          ...agent,
          id: 'agt_starting',
          title: 'Starting Agent',
        })),
      })),
    }));
    const mixed = [...items({ status: 'working' }), ...starting];
    const { rerender } = render(tree({ items: mixed, statusFilter: 'all' }));
    expect(screen.getByText('1 update failed · Fix').closest('details')).not.toHaveAttribute(
      'open',
    );
    await userEvent.click(screen.getByText('1 update failed · Fix'));
    expect(
      screen.getByRole('treeitem', { name: /Agent one.*Runtime update failed/ }),
    ).toBeVisible();
    expect(screen.getByRole('treeitem', { name: /Starting Agent.*Preparing/ })).toBeVisible();

    await userEvent.click(screen.getByText('1 update failed · Fix'));
    rerender(tree({ items: mixed, statusFilter: 'needs_attention' }));
    const failedAgent = screen.getByRole('treeitem');
    expect(failedAgent).toHaveAccessibleName(/Agent one.*Runtime update failed/);
    expect(failedAgent).toBeVisible();

    rerender(tree({ items: mixed, statusFilter: 'starting' }));
    expect(screen.getByRole('treeitem')).toHaveAccessibleName(/Starting Agent.*Preparing/);
  });

  it('reveals a different selected Agent after the failed environment disclosure is closed', async () => {
    const user = userEvent.setup();
    const twoAgents = items({ id: 'agt_one' }).map((item) => ({
      ...item,
      workspaces: item.workspaces.map((workspace) => ({
        ...workspace,
        agents: [
          workspace.agents[0]!,
          { ...workspace.agents[0]!, id: 'agt_two', title: 'Agent two' },
        ],
      })),
    }));
    const draw = (agentId: string) => tree({ items: twoAgents, route: { kind: 'agent', agentId } });
    const view = render(draw('agt_one'));
    const selectedAgent = (name: string) => screen.getByText(name).closest('[role="treeitem"]');
    expect(selectedAgent('Agent one')).toHaveAttribute('aria-selected', 'true');
    expect(selectedAgent('Agent one')).toBeVisible();

    await user.click(screen.getByText('1 update failed · Fix'));
    expect(selectedAgent('Agent one')).not.toBeVisible();

    view.rerender(draw('agt_two'));
    expect(selectedAgent('Agent two')).toHaveAttribute('aria-selected', 'true');
    expect(selectedAgent('Agent two')).toBeVisible();
  });

  it.each(['failed', 'stopped'] as const)(
    'keeps historical %s outcomes and their date group',
    (taskStatus) => {
      render(tree({ items: items({ taskStatus }) }));
      const row = within(screen.getByRole('group', { name: 'Earlier' })).getByRole('treeitem');
      expect(row).toHaveTextContent(taskStatus === 'failed' ? 'Failed' : 'Response interrupted');
      expect(row).toHaveTextContent('Runtime update failed');
      expect(screen.queryByRole('group', { name: 'Active' })).not.toBeInTheDocument();
    },
  );

  it.each([undefined, 'queued', 'updating'] as const)(
    'does not infer failure from update state %s',
    (state) => {
      render(
        tree({ items: items({}, { runtimeUpdate: state ? { ...update, state } : undefined }) }),
      );
      const row = within(screen.getByRole('group', { name: 'Updating environment' })).getByRole(
        'treeitem',
      );
      expect(row).toHaveTextContent('Updating runtime');
      expect(row).toHaveTextContent('Turn ended');
      expect(screen.queryByText('Runtime update failed')).not.toBeInTheDocument();
    },
  );

  it('does not treat a failed request on an otherwise active environment as parked rotation', () => {
    render(
      tree({
        items: items(
          {},
          { status: 'active', lifecycle: navigationFixture()[0]!.sandbox.lifecycle },
        ),
      }),
    );
    expect(screen.getByRole('group', { name: 'Earlier' })).toBeInTheDocument();
    expect(screen.getByRole('treeitem')).toHaveAccessibleName(/Turn ended/);
    expect(screen.queryByText('Runtime update failed')).not.toBeInTheDocument();
  });

  it('holds stale observations outside Active and removes the running timer', () => {
    const { container } = render(
      tree({ items: navigationFixture('active', 'working'), refreshError: 'Refresh unavailable' }),
    );
    expect(screen.getByRole('group', { name: 'Status unavailable' })).toBeInTheDocument();
    expect(screen.getByRole('treeitem')).toHaveTextContent('Status unavailable');
    expect(container.querySelector('.task-running')).toBeNull();
    expect(screen.queryByRole('group', { name: 'Active' })).not.toBeInTheDocument();
  });

  it.each(['retiring', 'retired', 'accepted'] as const)(
    'prioritizes %s deletion over stale failed rotation',
    (state) => {
      render(
        tree({
          items: items({}, state === 'accepted' ? {} : { status: state }),
          deletingSandboxIds: state === 'accepted' ? new Set(['sbx_1']) : undefined,
          refreshError: 'Refresh unavailable',
        }),
      );
      expect(screen.queryByRole('treeitem')).not.toBeInTheDocument();
      expect(screen.queryByRole('group', { name: 'Needs attention' })).not.toBeInTheDocument();
      if (state === 'retired') expect(screen.queryByText(/Cleaning up/)).not.toBeInTheDocument();
      else expect(screen.getByText(/Cleaning up/)).toBeInTheDocument();
    },
  );

  it('starts a new task clock after a fresh active observation', () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-11-19T04:00:00Z'));
    const view = render(tree());
    view.rerender(
      tree({
        items: items(
          { status: 'working', taskStartedAt: new Date().toISOString() },
          {
            status: 'active',
            lifecycle: navigationFixture()[0]!.sandbox.lifecycle,
            runtimeUpdate: { ...update, state: 'up_to_date' },
          },
        ),
      }),
    );
    expect(screen.getByRole('group', { name: 'Active' })).toHaveTextContent('<1m');
    act(() => vi.advanceTimersByTime(60_000));
    expect(screen.getByRole('group', { name: 'Active' })).toHaveTextContent('1m');
    expect(screen.queryByText('Runtime update failed')).not.toBeInTheDocument();
  });
});
