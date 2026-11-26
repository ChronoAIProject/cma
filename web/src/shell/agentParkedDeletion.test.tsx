import '../test/connectedAccounts';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { AGENT_DETAIL, AGENT_TRANSCRIPT } from '../test/agent';
import { parkedRuntimeResultFixture, workspaceFixture } from '../test/hierarchy';
import { AgentSurface } from '../agent/AgentSurface';
import { controller } from '../agent/AgentSurface.fixture';
import { conversationFromTranscript } from '../agent/conversation';
import { Tree } from './Tree';

vi.mock('@novnc/novnc', () => ({
  default: class extends EventTarget {
    disconnect() {}
  },
}));

const permissions = {
  createWorkspace: false,
  createAgent: false,
  pauseSandbox: true,
  deleteSandbox: true,
  removeWorkspace: false,
  stopAgent: true,
  deleteAgent: true,
  exportHistory: true,
};

function environment(id: string, purpose: 'image_rotation' | 'revive') {
  const base = parkedRuntimeResultFixture(purpose);
  const sandbox = {
    ...base,
    id,
    owner: 'owner',
    lifecycle: {
      ...base.lifecycle!,
      result: { ...base.lifecycle!.result!, actions: ['observe', 'retire'] as const },
    },
  };
  const workspace = {
    ...workspaceFixture('working'),
    id: `workspace-${id}`,
    sandboxId: id,
    agents: workspaceFixture('working').agents.map((agent) => ({
      ...agent,
      id: `agent-${id}`,
      title: `Task ${id}`,
      sandboxId: id,
      workspaceId: `workspace-${id}`,
    })),
  };
  return { sandbox, workspaces: [workspace] };
}

it.each(['agent', 'sandbox'] as const)(
  'dispatches whole-environment deletion from the %s tree to each current parent',
  (view) => {
    const items = [
      environment('parent-one', 'image_rotation'),
      environment('parent-two', 'revive'),
    ];
    const onAction = vi.fn();
    render(
      <Tree
        view={view}
        viewerId="owner"
        items={items}
        route={{ kind: 'home' }}
        permissions={permissions}
        onNavigate={vi.fn()}
        onAction={onAction}
      />,
    );
    for (const { sandbox } of items) {
      const row = document.querySelector(`[data-node-id="agent:agent-${sandbox.id}"]`)!;
      fireEvent.click(within(row as HTMLElement).getByRole('button', { name: /^Options for/ }));
      const remove = screen.getByRole('menuitem', { name: /^Delete environment…/ });
      expect(remove).not.toHaveAttribute('aria-disabled', 'true');
      const agentRemove = screen.getByRole('menuitem', { name: /^Delete (?!environment)/ });
      expect(agentRemove).toHaveAttribute('aria-disabled', 'true');
      expect(agentRemove).toHaveTextContent('Retry runtime update');
      expect(screen.getByRole('menuitem', { name: /^Stop/ })).toHaveTextContent(
        'Delete environment',
      );
      fireEvent.click(remove);
      expect(onAction).toHaveBeenLastCalledWith({ kind: 'delete_sandbox', sandboxId: sandbox.id });
    }
    expect(onAction).toHaveBeenCalledTimes(2);
  },
);

it.each(['agent', 'sandbox'] as const)(
  'withdraws the %s tree environment action when the parent no longer matches',
  (view) => {
    const item = environment('parent-one', 'image_rotation');
    const props = {
      view,
      viewerId: 'owner',
      route: { kind: 'home' } as const,
      permissions,
      onNavigate: vi.fn(),
      onAction: vi.fn(),
    };
    const drawn = render(<Tree {...props} items={[item]} />);
    const menu = screen.getByRole('button', { name: 'Options for Task parent-one' });
    fireEvent.click(menu);
    expect(screen.getByRole('menuitem', { name: /^Delete environment…/ })).toBeVisible();
    const workspace = item.workspaces[0]!;
    drawn.rerender(
      <Tree
        {...props}
        items={[
          {
            ...item,
            workspaces: [
              {
                ...workspace,
                agents: workspace.agents.map((agent) => ({
                  ...agent,
                  sandboxId: 'different-parent',
                })),
              },
            ],
          },
        ]}
      />,
    );
    expect(
      screen.queryByRole('menuitem', { name: /^Delete environment…/ }),
    ).not.toBeInTheDocument();
    expect(props.onAction).not.toHaveBeenCalled();
  },
);

it('dispatches header deletion using the observed current parent and rejects a later mismatched observation', () => {
  const onAction = vi.fn();
  const current = environment('header-parent', 'image_rotation');
  const other = environment('other-parent', 'image_rotation');
  const renderSurface = (observed: typeof current.sandbox) => (
    <AgentSurface
      agentId={AGENT_DETAIL.id}
      viewerId="owner"
      controller={controller(
        {
          status: 'ready',
          detail: {
            ...AGENT_DETAIL,
            sandbox: { ...AGENT_DETAIL.sandbox, id: current.sandbox.id },
            worktree: { ...AGENT_DETAIL.worktree, sandboxId: current.sandbox.id },
          },
          workspace: current.workspaces[0]!,
          conversation: conversationFromTranscript(AGENT_TRANSCRIPT),
        },
        undefined,
        { progressObservation: { sandbox: observed, stale: false } },
      )}
      detailsOpen={false}
      onDetailsOpenChange={vi.fn()}
      onNavigate={vi.fn()}
      permissions={permissions}
      onAgentAction={onAction}
    />
  );
  const drawn = render(renderSurface(current.sandbox));
  fireEvent.click(screen.getByRole('button', { name: 'More Agent actions' }));
  expect(screen.getByRole('menuitem', { name: /^Delete task/ })).toHaveAttribute(
    'aria-disabled',
    'true',
  );
  fireEvent.click(screen.getByRole('menuitem', { name: /^Delete environment…/ }));
  expect(onAction).toHaveBeenCalledExactlyOnceWith({
    kind: 'delete_sandbox',
    sandboxId: 'header-parent',
  });
  fireEvent.click(screen.getByRole('button', { name: 'More Agent actions' }));
  drawn.rerender(renderSurface(other.sandbox));
  expect(screen.queryByRole('menuitem', { name: /^Delete environment…/ })).not.toBeInTheDocument();
  expect(onAction).toHaveBeenCalledOnce();
});
