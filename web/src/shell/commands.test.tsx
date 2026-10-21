import { describe, expect, it, vi } from 'vitest';
import { navigationFixture, sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { shellCommands, type ShellCommandOptions } from './commands';

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
              { ...first, id: 'agt_old', updatedAt: '2026-09-01T00:00:00Z' },
              { ...first, id: 'agt_new', updatedAt: '2026-09-09T00:00:00Z' },
              { ...first, id: 'agt_bad', updatedAt: 'not a date' },
            ],
          },
        ],
      },
    ];
    const ids = shellCommands(options({ items }))
      .filter(({ group }) => group === 'Agents')
      .map(({ id }) => id);
    expect(ids).toEqual(['agent:agt_new', 'agent:agt_old', 'agent:agt_bad']);
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
