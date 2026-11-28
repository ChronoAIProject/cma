import '../test/connectedAccounts';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, agentClient } from '../test/agent';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { AgentSurface } from './AgentSurface';
import { controller } from './AgentSurface.fixture';
import { conversationFromTranscript } from './conversation';
import type { AgentController } from './controller';
import type { SandboxDetail, SandboxStatus } from '../api/types';

const permissions = {
  createWorkspace: true,
  createAgent: true,
  pauseSandbox: true,
  deleteSandbox: true,
  removeWorkspace: true,
  stopAgent: true,
  deleteAgent: true,
  exportHistory: true,
};

function surface(
  status: SandboxStatus = 'installing',
  overrides: {
    parent?: Partial<SandboxDetail>;
    stale?: boolean;
    accessUncertain?: boolean;
    viewerId?: string;
    deleteSandbox?: boolean;
    missing?: boolean;
    agentId?: string;
  } = {},
) {
  const sandbox = { ...sandboxFixture(status), owner: 'owner', ...overrides.parent };
  const detail = {
    ...AGENT_DETAIL,
    status: 'starting' as const,
    readyAt: undefined,
    sandbox: { ...AGENT_DETAIL.sandbox, status },
  };
  const observed: AgentController['progressObservation'] = overrides.missing
    ? undefined
    : { sandbox, stale: overrides.stale ?? false };
  const controls = controller(
    {
      status: 'ready',
      detail,
      workspace: workspaceFixture('starting'),
      conversation: conversationFromTranscript(AGENT_TRANSCRIPT),
      historyLoading: true,
    },
    undefined,
    { progressObservation: observed, controlBaselineReady: false },
  );
  const onNavigate = vi.fn(),
    onAgentAction = vi.fn();
  const props = {
    agentId: overrides.agentId ?? detail.id,
    viewerId: overrides.viewerId ?? 'owner',
    controller: controls,
    accessUncertain: overrides.accessUncertain,
    permissions: { ...permissions, deleteSandbox: overrides.deleteSandbox ?? true },
    detailsOpen: false,
    onDetailsOpenChange: vi.fn(),
    onNavigate,
    onAgentAction,
  };
  const draw = () => (
    <CmaClientProvider client={agentClient()}>
      <AgentSurface {...props} />
    </CmaClientProvider>
  );
  return { ...render(draw()), props, draw, onNavigate, onAgentAction, controls };
}

it.each(['provisioning', 'installing'] as const)(
  'offers only navigation from the %s panel and header while activity is loading',
  (status) => {
    const { onNavigate, onAgentAction, controls } = surface(status);
    const panel = screen.getByRole('region', { name: 'Cancel Agent setup' });
    expect(panel).toHaveTextContent('This removes the environment and all Agents inside it.');
    fireEvent.click(within(panel).getByRole('button', { name: 'Cancel setup…' }));
    expect(onNavigate).toHaveBeenCalledExactlyOnceWith({ kind: 'sandbox', sandboxId: 'sbx_1' });
    fireEvent.click(screen.getByRole('button', { name: 'More Agent actions' }));
    expect(screen.getByRole('menuitem', { name: /^Stop agent/ })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
    expect(screen.getByRole('menuitem', { name: /^Delete task/ })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
    const review = screen.getByRole('menuitem', { name: /^Review environment/ });
    expect(review).not.toHaveAttribute('aria-disabled', 'true');
    fireEvent.click(review);
    expect(onNavigate).toHaveBeenCalledTimes(2);
    expect(onNavigate).toHaveBeenLastCalledWith({ kind: 'sandbox', sandboxId: 'sbx_1' });
    expect(onAgentAction).not.toHaveBeenCalled();
    expect(controls.stop).not.toHaveBeenCalled();
    expect(controls.delete).not.toHaveBeenCalled();
  },
);

it.each([
  { stale: true },
  { missing: true },
  { accessUncertain: true },
  { deleteSandbox: false },
  { viewerId: 'other-owner' },
  { parent: { owner: undefined } },
  { parent: { id: 'sbx_other' } },
])('does not offer a setup deletion path without verified access: %j', (overrides) => {
  surface('installing', overrides);
  expect(screen.queryByRole('button', { name: 'Cancel setup…' })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'More Agent actions' }));
  expect(screen.queryByRole('menuitem', { name: /^Review environment/ })).not.toBeInTheDocument();
});

it.each(['active', 'paused', 'reviving', 'retiring', 'retired'] as const)(
  'does not describe %s as cancellable setup',
  (status) => {
    surface(status);
    expect(screen.queryByRole('region', { name: 'Cancel Agent setup' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'More Agent actions' }));
    expect(screen.queryByRole('menuitem', { name: /^Review environment/ })).not.toBeInTheDocument();
  },
);

it('drops the previous parent action when the selected Agent changes before its detail arrives', () => {
  const view = surface();
  expect(screen.getByRole('button', { name: 'Cancel setup…' })).toBeInTheDocument();
  view.props.agentId = 'agt_next';
  view.rerender(view.draw());
  expect(screen.queryByRole('button', { name: 'Cancel setup…' })).not.toBeInTheDocument();
  expect(view.onNavigate).not.toHaveBeenCalled();
});

it('withdraws the setup shortcut when permission is lost with the menu open', () => {
  const view = surface();
  fireEvent.click(screen.getByRole('button', { name: 'More Agent actions' }));
  expect(screen.getByRole('menuitem', { name: /^Review environment/ })).toBeEnabled();
  view.props.permissions.deleteSandbox = false;
  view.rerender(view.draw());
  expect(screen.queryByRole('menuitem', { name: /^Review environment/ })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Cancel setup…' })).not.toBeInTheDocument();
  expect(view.onNavigate).not.toHaveBeenCalled();
  expect(view.onAgentAction).not.toHaveBeenCalled();
});
