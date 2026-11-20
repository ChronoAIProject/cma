import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { navigationFixture } from '../test/hierarchy';
import { activityLabel } from './Tree';
import { agentSignal } from './treePresentation';
import { renderTree } from './Tree.test-support';

describe('Sandbox navigation tree', () => {
  it('uses saved setup provenance for unselected rows without loading their conversations', () => {
    const items = navigationFixture().map((item) => ({
      ...item,
      workspaces: item.workspaces.map((workspace) => ({
        ...workspace,
        agents: workspace.agents.map((agent) => ({
          ...agent,
          title: 'Configured agent',
          runtimeMetadata: {
            description: 'Automatic setup preview',
            descriptionSource: 'prompt_preview' as const,
            descriptionIsSetupPreview: true,
          },
        })),
      })),
    }));
    renderTree({ items, route: { kind: 'home' } });
    expect(screen.getByRole('link', { name: /Configured agent/ })).toBeInTheDocument();
    expect(screen.queryByText('Automatic setup preview')).not.toBeInTheDocument();
  });

  it('uses the selected Agent runtime directory and description without guessing another Agent cwd', () => {
    const items = navigationFixture().map((item) => ({
      ...item,
      workspaces: item.workspaces.map((workspace) => ({
        ...workspace,
        agents: workspace.agents.map((agent) => ({
          ...agent,
          runtimeMetadata: {
            workingDirectory: '/home/cma/project-from-codex',
            description: 'Investigate the failing deployment',
          },
        })),
      })),
    }));
    const selectedId = items[0]!.workspaces[0]!.agents[0]!.id;
    const view = renderTree({ items, route: { kind: 'agent', agentId: selectedId } });
    expect(screen.getByText('/home/cma/project-from-codex')).toHaveAttribute(
      'title',
      '/home/cma/project-from-codex',
    );
    expect(screen.getByRole('link', { name: /Agent one/ })).toBeInTheDocument();
    expect(screen.queryByText('Investigate the failing deployment')).not.toBeInTheDocument();
    view.unmount();
    renderTree({ items });
    expect(screen.getByText('/workspaces/chrono-bot')).toBeInTheDocument();
    expect(screen.queryByText('/home/cma/project-from-codex')).not.toBeInTheDocument();
  });

  it('renders Sandbox, Workspace, and Agent rows without a Worktree navigation node', () => {
    renderTree();
    expect(screen.getByText('Atlas')).toBeInTheDocument();
    expect(screen.getByText('/workspaces/chrono-bot')).toBeInTheDocument();
    const agent = screen.getByText('Agent one').closest('[role="treeitem"]');
    expect(agent).not.toHaveAttribute('data-dimmed');
    expect(within(agent as HTMLElement).getByText('Ready')).toBeInTheDocument();
    expect(screen.queryByText('agent/one')).not.toBeInTheDocument();
    // The Agent leads with its still, settled character rather than a generic glyph.
    const character = agent?.querySelector('.tree-row__icon .character-avatar');
    expect(character).toHaveAttribute('data-pose', 'idle');
    expect(character).toHaveAttribute('data-motion', 'still');
    expect(character).toHaveStyle({ width: '20px', height: '20px' });
  });

  it('dresses each Agent in its Sandbox colour', () => {
    renderTree({
      items: navigationFixture().map((item) => ({
        ...item,
        sandbox: { ...item.sandbox, color: '#2f96fa' },
      })),
    });
    const agent = screen.getByText('Agent one').closest('[role="treeitem"]');
    const character = agent?.querySelector<HTMLElement>('.tree-row__icon .character-avatar');
    expect(character?.style.color).toBe('rgb(47, 150, 250)');
  });

  it('lets approval and failure colours replace the Sandbox colour', () => {
    for (const [status, tone] of [
      ['waiting_approval', 'warning'],
      ['failed', 'error'],
    ] as const) {
      const { container, unmount } = render(<>{agentSignal(status, 'agt_1', 20, '#2f96fa')}</>);
      const character = container.querySelector<HTMLElement>('.character-avatar');
      expect(character).toHaveAttribute('data-tone', tone);
      expect(character?.style.color).toBe('');
      unmount();
    }
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

describe('Flat Agent navigation', () => {
  it('interleaves Agents from different parents by recent activity without parent rows', async () => {
    const base = navigationFixture()[0]!;
    const workspace = base.workspaces[0]!;
    const agent = workspace.agents[0]!;
    const onNavigate = vi.fn();
    const items = [
      {
        ...base,
        workspaces: [
          {
            ...workspace,
            agents: [
              {
                ...agent,
                id: 'older',
                title: 'Older Agent',
                taskActivityAt: '2026-10-20T08:00:00Z',
              },
              {
                ...agent,
                id: 'newest',
                title: 'Newest Agent',
                taskActivityAt: '2026-10-22T08:00:00Z',
              },
              { ...agent, id: 'gone', title: 'Deleted Agent', status: 'deleted' as const },
            ],
          },
        ],
      },
      {
        sandbox: { ...base.sandbox, id: 'sbx_other', name: 'Other Sandbox' },
        workspaces: [
          {
            ...workspace,
            id: 'wsp_other',
            name: 'Other Workspace',
            agents: [
              {
                ...agent,
                id: 'middle',
                title: 'Middle Agent',
                taskActivityAt: '2026-10-21T08:00:00Z',
              },
            ],
          },
        ],
      },
    ];
    renderTree({ items, view: 'agent', onNavigate });
    const tree = screen.getByRole('tree', { name: 'Personal Agents' });
    const rows = within(tree).getAllByRole('treeitem');
    expect(rows.map((row) => row.dataset.nodeId)).toEqual([
      'agent:newest',
      'agent:middle',
      'agent:older',
    ]);
    expect(within(tree).getByRole('group', { name: 'Earlier' })).toBeInTheDocument();
    for (const name of [
      'Atlas',
      'Other Sandbox',
      'Other Workspace',
      '/workspaces/chrono-bot',
      'Deleted Agent',
    ])
      expect(within(tree).queryByText(name)).not.toBeInTheDocument();
    for (const row of rows) {
      expect(row).toHaveAttribute('aria-level', '1');
      expect(row).not.toHaveAttribute('aria-expanded');
    }
    rows[0]!.focus();
    await userEvent.keyboard('{ArrowDown}{Enter}');
    expect(onNavigate).toHaveBeenCalledExactlyOnceWith({ kind: 'agent', agentId: 'middle' });
  });

  it('preserves parent pause status and action barriers without grouping', async () => {
    renderTree({ items: navigationFixture('paused'), view: 'agent', statusFilter: 'paused' });
    const tree = screen.getByRole('tree', { name: 'Personal Agents' });
    expect(within(tree).getByRole('link', { name: /Agent one/ })).toHaveTextContent('Paused');
    expect(within(tree).getByRole('treeitem')).toHaveAttribute('data-dimmed');
    await userEvent.click(within(tree).getByRole('button', { name: 'Options for Agent one' }));
    expect(screen.getByRole('menuitem', { name: /^Stop/ })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
    expect(screen.getByRole('menuitem', { name: 'Export retained conversation' })).toHaveAttribute(
      'aria-disabled',
      'false',
    );
  });

  it('keeps Sandboxes and Workspaces in Sandbox view', () => {
    renderTree({ view: 'sandbox' });
    expect(screen.getByText('Atlas')).toBeInTheDocument();
    expect(screen.getByText('/workspaces/chrono-bot')).toBeInTheDocument();
    expect(screen.getByText('Agent one')).toBeInTheDocument();
  });
});
