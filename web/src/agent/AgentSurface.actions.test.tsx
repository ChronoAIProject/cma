import '../test/connectedAccounts';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { AGENT_DETAIL, AGENT_TRANSCRIPT } from '../test/agent';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import type { SandboxStatus } from '../api/types';
import { conversationFromTranscript } from './conversation';
import { AgentSurface } from './AgentSurface';
import { controller } from './AgentSurface.fixture';

vi.mock('@novnc/novnc', () => ({
  default: class extends EventTarget {
    disconnect() {}
  },
}));

const ALL_ACTIONS = {
  createWorkspace: true,
  createAgent: true,
  pauseSandbox: true,
  deleteSandbox: true,
  removeWorkspace: true,
  stopAgent: true,
  deleteAgent: true,
  exportHistory: true,
};

it.each([
  [true, false, 'Pausing'],
  [false, false, 'Ready'],
  [true, true, 'Ready'],
] as const)(
  'uses only a fresh, matching parent operation in the heading',
  (matches, stale, label) => {
    const sandbox = sandboxFixture();
    render(
      <AgentSurface
        agentId={AGENT_DETAIL.id}
        viewerId="owner"
        controller={controller(
          {
            status: 'ready',
            detail: AGENT_DETAIL,
            workspace: workspaceFixture(),
            conversation: conversationFromTranscript(AGENT_TRANSCRIPT),
          },
          undefined,
          {
            progressObservation: {
              sandbox: {
                ...sandbox,
                id: matches ? AGENT_DETAIL.sandbox.id : 'sbx_other',
                owner: 'owner',
                lifecycle: {
                  ...sandbox.lifecycle!,
                  operation: {
                    purpose: 'pause',
                    phase: 'checkpoint',
                    startedAt: sandbox.createdAt,
                  },
                },
              },
              stale,
            },
          },
        )}
        detailsOpen={false}
        onDetailsOpenChange={vi.fn()}
        onNavigate={vi.fn()}
      />,
    );
    expect(within(screen.getByRole('banner')).getByText(label, { exact: true })).toBeVisible();
  },
);

it.each([false, true])(
  'offers owner restart only when the server advertises it (%s)',
  async (restartAvailable) => {
    const restart = vi.fn(async () => undefined);
    const actor = userEvent.setup();
    render(
      <AgentSurface
        agentId="agt_1"
        viewerId="owner"
        controller={controller(
          {
            status: 'ready',
            detail: { ...AGENT_DETAIL, status: 'stopped', restartAvailable },
            workspace: workspaceFixture(),
            conversation: conversationFromTranscript(AGENT_TRANSCRIPT),
          },
          undefined,
          { restart },
        )}
        detailsOpen={false}
        onDetailsOpenChange={vi.fn()}
        onNavigate={vi.fn()}
        permissions={ALL_ACTIONS}
      />,
    );
    const action = screen.queryByRole('button', { name: 'Restart Agent' });
    if (restartAvailable) await actor.click(action!);
    else expect(action).toBeNull();
    expect(restart).toHaveBeenCalledTimes(Number(restartAvailable));
  },
);

it('offers Stop, Export and Delete for a running Agent owned by the viewer', async () => {
  const onAgentAction = vi.fn();
  const actor = userEvent.setup();
  const ready = controller(
    {
      status: 'ready',
      detail: AGENT_DETAIL,
      workspace: workspaceFixture(),
      conversation: conversationFromTranscript(AGENT_TRANSCRIPT),
    },
    undefined,
    { progressObservation: { sandbox: { ...sandboxFixture(), owner: 'owner' }, stale: false } },
  );
  render(
    <AgentSurface
      agentId="agt_1"
      viewerId="owner"
      controller={ready}
      detailsOpen={false}
      onDetailsOpenChange={vi.fn()}
      onNavigate={vi.fn()}
      permissions={ALL_ACTIONS}
      onAgentAction={onAgentAction}
    />,
  );
  await actor.click(screen.getByRole('button', { name: 'More Agent actions' }));
  expect(
    screen.getByRole('menuitem', {
      name: 'Delete task',
    }),
  ).toHaveAttribute('aria-disabled', 'false');
  expect(screen.getByRole('menuitem', { name: 'Export raw history' })).toBeEnabled();
  const rows = screen.getAllByRole('menuitem');
  for (const [index, label] of [
    'Rename',
    'Update system prompt',
    'Update skills',
    'Export retained conversation',
    'Export raw history',
    'View deletion status',
    'Stop agent',
    'Delete task',
  ].entries())
    expect(rows[index]).toHaveAccessibleName(expect.stringContaining(label));
  expect(screen.getByRole('menu').querySelectorAll('[role="separator"]')).toHaveLength(1);
  expect(onAgentAction).not.toHaveBeenCalled();
  await actor.click(screen.getByRole('menuitem', { name: 'Stop agent' }));
  expect(onAgentAction).toHaveBeenCalledExactlyOnceWith({ kind: 'stop_agent', agentId: 'agt_1' });
  for (const [label, kind] of [
    ['Delete task', 'delete_agent'],
    ['Export raw history', 'export_history'],
    ['Export retained conversation', 'export_retained_conversation'],
  ] as const) {
    onAgentAction.mockClear();
    await actor.click(screen.getByRole('button', { name: 'More Agent actions' }));
    await actor.click(screen.getByRole('menuitem', { name: label }));
    expect(onAgentAction).toHaveBeenCalledExactlyOnceWith({ kind, agentId: 'agt_1' });
  }
});

it('holds every header action while another action is being confirmed', async () => {
  const onAgentAction = vi.fn();
  const actor = userEvent.setup();
  render(
    <AgentSurface
      agentId="agt_1"
      controller={controller(
        {
          status: 'ready',
          detail: AGENT_DETAIL,
          workspace: workspaceFixture(),
          conversation: conversationFromTranscript(AGENT_TRANSCRIPT),
        },
        undefined,
        { progressObservation: { sandbox: sandboxFixture(), stale: false } },
      )}
      detailsOpen={false}
      onDetailsOpenChange={vi.fn()}
      onNavigate={vi.fn()}
      permissions={ALL_ACTIONS}
      actionsBusy
      onAgentAction={onAgentAction}
    />,
  );
  await actor.click(screen.getByRole('button', { name: 'More Agent actions' }));
  const stop = screen.getByRole('menuitem', {
    name: 'Delete task Another action is still being confirmed.',
  });
  await actor.click(stop);
  expect(onAgentAction).not.toHaveBeenCalled();
});

it('offers only the Details toggle and creation origin to a reader without actions', async () => {
  render(
    <AgentSurface
      agentId="agt_1"
      controller={controller({
        status: 'ready',
        detail: AGENT_DETAIL,
        workspace: workspaceFixture(),
        conversation: conversationFromTranscript(AGENT_TRANSCRIPT),
      })}
      detailsOpen={false}
      onDetailsOpenChange={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
  await userEvent.setup().click(screen.getByRole('button', { name: 'More Agent actions' }));
  expect(screen.getAllByRole('menuitem')).toHaveLength(6);
  for (const item of screen.getAllByRole('menuitem'))
    expect(item).toHaveAttribute('aria-disabled', 'true');
  expect(screen.getByRole('button', { name: 'Show Agent details' })).toBeEnabled();
});

it('omits management actions and parent links for a shared reader with account permissions', async () => {
  const onAgentAction = vi.fn();
  render(
    <AgentSurface
      agentId="agt_1"
      viewerId="reader"
      controller={controller(
        {
          status: 'ready',
          detail: { ...AGENT_DETAIL, status: 'stopped' },
          workspace: workspaceFixture(),
          conversation: conversationFromTranscript(AGENT_TRANSCRIPT),
        },
        undefined,
        {
          progressObservation: {
            sandbox: { ...sandboxFixture('paused'), owner: 'another-owner' },
            stale: false,
          },
        },
      )}
      detailsOpen={false}
      onDetailsOpenChange={vi.fn()}
      onNavigate={vi.fn()}
      permissions={ALL_ACTIONS}
      onAgentAction={onAgentAction}
    />,
  );
  await userEvent.click(screen.getByRole('button', { name: 'More Agent actions' }));
  expect(screen.queryByRole('menuitem', { name: /^Stop agent/ })).not.toBeInTheDocument();
  expect(screen.queryByRole('menuitem', { name: /^Delete task/ })).not.toBeInTheDocument();
  expect(
    screen.queryByRole('menuitem', { name: 'Open Sandbox to resume' }),
  ).not.toBeInTheDocument();
  expect(onAgentAction).not.toHaveBeenCalled();
});

it.each<{
  status: SandboxStatus;
  agent: 'stopped' | 'failed';
  stale?: boolean;
  busy?: boolean;
  resuming?: boolean;
  reason?: string;
  link?: boolean;
}>([
  { status: 'paused', agent: 'stopped', link: true, reason: 'Resume the Sandbox' },
  { status: 'paused', agent: 'failed', link: true, reason: 'Resume the Sandbox' },
  { status: 'paused', agent: 'stopped', stale: true, reason: 'Sandbox status is unavailable' },
  { status: 'paused', agent: 'stopped', busy: true, reason: 'Another action' },
  { status: 'paused', agent: 'stopped', resuming: true, reason: 'finish resuming' },
  { status: 'reviving', agent: 'stopped', reason: 'finish resuming' },
  { status: 'retiring', agent: 'failed', reason: 'being deleted' },
  { status: 'active', agent: 'stopped' },
])(
  'uses current parent state for header deletion (%j)',
  async ({ status, agent, stale, busy, resuming, reason, link }) => {
    const onNavigate = vi.fn();
    const onAgentAction = vi.fn();
    const sandbox = sandboxFixture(status);
    render(
      <AgentSurface
        agentId={AGENT_DETAIL.id}
        viewerId="owner"
        controller={controller(
          {
            status: 'ready',
            detail: { ...AGENT_DETAIL, status: agent },
            workspace: workspaceFixture(agent),
            conversation: conversationFromTranscript(AGENT_TRANSCRIPT),
          },
          undefined,
          {
            progressObservation: {
              sandbox: {
                ...sandbox,
                owner: 'owner',
                lifecycle: resuming
                  ? {
                      ...sandbox.lifecycle!,
                      operation: {
                        purpose: 'revive',
                        phase: 'home_restore',
                        startedAt: sandbox.createdAt,
                      },
                    }
                  : sandbox.lifecycle,
              },
              stale: stale ?? false,
            },
          },
        )}
        detailsOpen={false}
        onDetailsOpenChange={vi.fn()}
        onNavigate={onNavigate}
        permissions={ALL_ACTIONS}
        actionsBusy={busy}
        onAgentAction={onAgentAction}
      />,
    );
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: 'More Agent actions' }));
    const remove = screen.getByRole('menuitem', { name: /^Delete task/ });
    expect(remove).toHaveAttribute('aria-disabled', String(reason !== undefined));
    if (reason) expect(remove).toHaveTextContent(reason);
    const resume = screen.queryByRole('menuitem', { name: 'Open Sandbox to resume' });
    if (link) {
      expect(remove).toHaveTextContent('and its Worktree');
      expect(resume).toHaveAttribute('href', '/sandboxes/sbx_1');
      await user.keyboard('{Home}{ArrowDown}{ArrowDown}{ArrowDown}{ArrowDown}{ArrowDown}');
      expect(resume).toHaveFocus();
      await user.keyboard('{Enter}');
      expect(onNavigate).toHaveBeenCalledExactlyOnceWith({ kind: 'sandbox', sandboxId: 'sbx_1' });
    } else {
      expect(resume).not.toBeInTheDocument();
      expect(onNavigate).not.toHaveBeenCalled();
    }
    expect(onAgentAction).not.toHaveBeenCalled();
  },
);
