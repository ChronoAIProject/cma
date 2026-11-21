import '../test/connectedAccounts';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { AGENT_DETAIL, AGENT_TRANSCRIPT } from '../test/agent';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
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

it('offers Stop, Export and Delete from the header, with the reason Delete is unavailable', async () => {
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
    { progressObservation: { sandbox: sandboxFixture(), stale: false } },
  );
  render(
    <AgentSurface
      agentId="agt_1"
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
      name: 'Delete task Stop this Agent before deleting it and its Worktree.',
    }),
  ).toHaveAttribute('aria-disabled', 'true');
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
  expect(screen.getAllByRole('menuitem')).toHaveLength(7);
  for (const item of screen.getAllByRole('menuitem'))
    expect(item).toHaveAttribute('aria-disabled', 'true');
  expect(screen.getByRole('button', { name: 'Show Agent details' })).toBeEnabled();
});

it('omits Stop for a shared reader even when account permissions include stopping owned Agents', async () => {
  const onAgentAction = vi.fn();
  render(
    <AgentSurface
      agentId="agt_1"
      viewerId="reader"
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
            sandbox: { ...sandboxFixture(), owner: 'another-owner' },
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
  expect(onAgentAction).not.toHaveBeenCalled();
});
