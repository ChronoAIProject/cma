import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { navigationFixture, sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { shellCommands, type ShellCommandOptions } from './commands';
import { CommandMenu, filterCommands } from './CommandMenu';

function options(overrides: Partial<ShellCommandOptions> = {}): ShellCommandOptions {
  return {
    items: navigationFixture(),
    onNavigate: vi.fn(),
    canOpenTriggers: false,
    canOpenOperations: false,
    onAppearance: vi.fn(),
    ...overrides,
  };
}

describe('shellCommands', () => {
  it.each(['Artifact', 'Artifacts'])(
    'opens Artifacts from command search for %s',
    async (query) => {
      const onNavigate = vi.fn();
      const user = userEvent.setup();
      render(
        <CommandMenu open onClose={vi.fn()} commands={shellCommands(options({ onNavigate }))} />,
      );
      expect(screen.getByRole('option', { name: 'Artifacts' })).toBeVisible();
      await user.type(screen.getByRole('combobox', { name: 'Search commands' }), query);
      expect(screen.getAllByRole('option')).toEqual([
        screen.getByRole('option', { name: 'Artifacts' }),
      ]);
      await user.keyboard('{Enter}');
      expect(onNavigate).toHaveBeenCalledExactlyOnceWith({ kind: 'artifacts' });
    },
  );

  it('lists Agents with where they live, then Sandboxes and Workspaces', () => {
    const list = shellCommands(options());
    const agent = list.find(({ id }) => id === 'agent:agt_1');
    expect(agent).toMatchObject({
      group: 'Agents',
      label: 'Agent one',
      meta: 'chrono-bot · Atlas',
      hint: 'Ready',
    });
    expect(list.find(({ id }) => id === 'sandbox:sbx_1')).toMatchObject({
      group: 'Sandboxes',
      label: 'Atlas',
      hint: 'Live',
    });
    expect(list.find(({ id }) => id === 'workspace:wsp_1')).toMatchObject({
      group: 'Workspaces',
      label: 'chrono-bot',
      meta: 'Atlas',
    });
  });

  it('navigates to the chosen resource', () => {
    const onNavigate = vi.fn();
    const list = shellCommands(options({ onNavigate }));
    list.find(({ id }) => id === 'agent:agt_1')?.run();
    list.find(({ id }) => id === 'sandbox:sbx_1')?.run();
    list.find(({ id }) => id === 'workspace:wsp_1')?.run();
    expect(onNavigate.mock.calls).toEqual([
      [{ kind: 'agent', agentId: 'agt_1' }],
      [{ kind: 'sandbox', sandboxId: 'sbx_1' }],
      [{ kind: 'workspace', workspaceId: 'wsp_1' }],
    ]);
  });

  it('finds distinct names and disambiguates identical Agent names by canonical ID', () => {
    const workspace = workspaceFixture();
    const first = workspace.agents[0]!;
    const onNavigate = vi.fn();
    const list = shellCommands(
      options({
        onNavigate,
        items: [
          {
            sandbox: sandboxFixture(),
            workspaces: [
              {
                ...workspace,
                agents: [
                  { ...first, id: 'agt_review_alpha', title: 'Review code' },
                  { ...first, id: 'agt_review_beta', title: 'Review code' },
                  { ...first, id: 'agt_plan_gamma', title: 'Plan rollout' },
                ],
              },
            ],
          },
        ],
      }),
    );
    expect(filterCommands(list, 'Review code')).toHaveLength(2);
    expect(filterCommands(list, 'Plan rollout').map(({ id }) => id)).toEqual([
      'agent:agt_plan_gamma',
    ]);
    for (const [query, id] of [
      ['agt_review_alpha', 'agt_review_alpha'],
      ['  AGT_REVIEW_BETA  ', 'agt_review_beta'],
      ['_alpha', 'agt_review_alpha'],
      ['Review _beta', 'agt_review_beta'],
    ]) {
      const matches = filterCommands(list, query!);
      expect(matches.map(({ id }) => id)).toEqual([`agent:${id}`]);
      matches[0]!.run();
      expect(onNavigate).toHaveBeenLastCalledWith({ kind: 'agent', agentId: id });
    }
    expect(filterCommands(list, 'Plan _beta')).toEqual([]);
  });

  it('searches each resource ID without indexing internal action or destination keys', () => {
    const onNavigate = vi.fn();
    const onNewAgent = vi.fn();
    const list = shellCommands(options({ onNavigate, onNewAgent }));
    for (const [id, route] of [
      ['agt_1', { kind: 'agent', agentId: 'agt_1' }],
      ['wsp_1', { kind: 'workspace', workspaceId: 'wsp_1' }],
      ['sbx_1', { kind: 'sandbox', sandboxId: 'sbx_1' }],
    ] as const) {
      const matches = filterCommands(list, id);
      expect(matches).toHaveLength(1);
      matches[0]!.run();
      expect(onNavigate).toHaveBeenLastCalledWith(route);
    }
    expect(filterCommands(list, 'Atlas Live').map(({ id }) => id)).toEqual(['sandbox:sbx_1']);
    expect(filterCommands(list, 'sandbox sbx_1').map(({ id }) => id)).toEqual(['sandbox:sbx_1']);
    for (const query of ['action:new-agent', 'go:agent-manager', 'agent:agt_1', 'agt_not_visible'])
      expect(filterCommands(list, query)).toEqual([]);
    expect(onNewAgent).not.toHaveBeenCalled();
  });

  it('keeps removed resources and their descendants out of ID search', () => {
    const workspace = workspaceFixture();
    const first = workspace.agents[0]!;
    const list = shellCommands(
      options({
        items: [
          {
            sandbox: sandboxFixture(),
            workspaces: [
              { ...workspace, agents: [{ ...first, id: 'agt_deleted', status: 'deleted' }] },
              {
                ...workspace,
                id: 'wsp_removed',
                status: 'removed',
                agents: [{ ...first, id: 'agt_removed_parent' }],
              },
            ],
          },
          ...(['retiring', 'retired'] as const).map((status) => ({
            sandbox: { ...sandboxFixture(status), id: `sbx_${status}` },
            workspaces: [
              { ...workspace, id: `wsp_${status}`, agents: [{ ...first, id: `agt_${status}` }] },
            ],
          })),
        ],
      }),
    );
    for (const id of [
      'agt_deleted',
      'wsp_removed',
      'agt_removed_parent',
      'sbx_retiring',
      'sbx_retired',
      'wsp_retiring',
      'wsp_retired',
      'agt_retiring',
      'agt_retired',
    ])
      expect(filterCommands(list, id)).toEqual([]);
    expect(filterCommands(shellCommands(options({ items: [] })), 'agt_1')).toEqual([]);
    expect(filterCommands(list, 'wsp_1')).toHaveLength(1);
  });

  it('orders Agents by most recent activity', () => {
    const workspace = workspaceFixture();
    const [first] = workspace.agents;
    if (first === undefined) throw new Error('fixture');
    const items = [
      {
        sandbox: sandboxFixture(),
        workspaces: [
          {
            ...workspace,
            agents: [
              {
                ...first,
                id: 'agt_old',
                taskActivityAt: '2026-09-01T00:00:00Z',
                updatedAt: '2026-11-20T00:00:00Z',
              },
              { ...first, id: 'agt_new', taskActivityAt: '2026-09-09T00:00:00Z' },
              { ...first, id: 'agt_z_missing', taskActivityAt: undefined },
              { ...first, id: 'agt_bad', taskActivityAt: 'not a date' },
              { ...first, id: 'agt_a_missing', taskActivityAt: undefined },
            ],
          },
        ],
      },
    ];
    const ids = shellCommands(options({ items }))
      .filter(({ group }) => group === 'Agents')
      .map(({ id }) => id);
    expect(ids).toEqual([
      'agent:agt_new',
      'agent:agt_old',
      'agent:agt_a_missing',
      'agent:agt_bad',
      'agent:agt_z_missing',
    ]);
  });

  it('leaves out deleted Agents, removed Workspaces and retiring Sandboxes', () => {
    const workspace = workspaceFixture('deleted');
    const items = [
      {
        sandbox: sandboxFixture(),
        workspaces: [workspace, { ...workspace, id: 'wsp_gone', status: 'removed' as const }],
      },
      { sandbox: { ...sandboxFixture('retiring'), id: 'sbx_gone' }, workspaces: [] },
    ];
    const ids = shellCommands(options({ items })).map(({ id }) => id);
    expect(ids).not.toContain('agent:agt_1');
    expect(ids).not.toContain('workspace:wsp_gone');
    expect(ids).not.toContain('sandbox:sbx_gone');
    expect(ids).toContain('workspace:wsp_1');
  });

  it('offers New agent, Triggers and Operations only when they can be used', () => {
    const without = shellCommands(options()).map(({ id }) => id);
    expect(without).not.toContain('action:new-agent');
    expect(without).not.toContain('go:triggers');
    expect(without).not.toContain('go:operations');
    const onNewAgent = vi.fn();
    const onNavigate = vi.fn();
    const all = shellCommands(
      options({ onNewAgent, onNavigate, canOpenTriggers: true, canOpenOperations: true }),
    );
    all.find(({ id }) => id === 'action:new-agent')?.run();
    all.find(({ id }) => id === 'go:triggers')?.run();
    all.find(({ id }) => id === 'go:operations')?.run();
    expect(onNewAgent).toHaveBeenCalledOnce();
    expect(onNavigate.mock.calls).toEqual([[{ kind: 'triggers' }], [{ kind: 'operations' }]]);
  });

  it('switches appearance', () => {
    const onAppearance = vi.fn();
    shellCommands(options({ onAppearance }))
      .find(({ id }) => id === 'action:appearance-dark')
      ?.run();
    expect(onAppearance).toHaveBeenCalledWith('dark');
  });
});
