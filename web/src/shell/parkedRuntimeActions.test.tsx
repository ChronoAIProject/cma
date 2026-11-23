import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import {
  parkedRuntimeResultFixture,
  parkedRuntimeUpdateFixture,
  workspaceFixture,
} from '../test/hierarchy';
import { agentActions } from './treeActions';
import { Tree } from './Tree';
import { AgentHeader } from '../agent/AgentHeader';
import { agentHeaderActionHandler, agentHeaderMenu } from '../agent/headerActions';

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

it.each(['image_rotation', 'revive'] as const)(
  'offers the existing retry controls for an owner list parked %s',
  (purpose) => {
    const sandbox = { ...parkedRuntimeResultFixture(purpose), owner: 'owner' };
    const onNavigate = vi.fn(),
      onAction = vi.fn();
    render(
      <Tree
        viewerId="owner"
        view="agent"
        items={[{ sandbox, workspaces: [workspaceFixture()] }]}
        route={{ kind: 'home' }}
        permissions={permissions}
        onNavigate={onNavigate}
        onAction={onAction}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /Options for/ }));
    const retry = screen.getByRole('menuitem', { name: /^Retry runtime update/ });
    expect(retry).not.toHaveAttribute('aria-disabled', 'true');
    expect(screen.getByRole('menuitem', { name: /^Stop/ })).toHaveTextContent(
      'Retry runtime update',
    );
    fireEvent.click(retry);
    expect(onNavigate).toHaveBeenCalledExactlyOnceWith({ kind: 'sandbox', sandboxId: sandbox.id });
    expect(onAction).not.toHaveBeenCalled();
  },
);

it('exposes the same runtime recovery entry in the task header without issuing a command', () => {
  const sandbox = { ...parkedRuntimeUpdateFixture(), owner: 'owner' };
  const onAction = vi.fn(),
    openSandbox = vi.fn();
  render(
    <AgentHeader
      title="Task"
      status="ready"
      detailsOpen={false}
      breadcrumb={[]}
      onDetailsOpenChange={vi.fn()}
      moreActions={agentHeaderMenu('ready', permissions, { sandbox, viewerId: 'owner' })}
      onMoreAction={agentHeaderActionHandler('agt_1', false, onAction, openSandbox)}
    />,
  );
  fireEvent.click(screen.getByRole('button', { name: 'More Agent actions' }));
  fireEvent.click(screen.getByRole('menuitem', { name: /^Retry runtime update/ }));
  expect(openSandbox).toHaveBeenCalledOnce();
  expect(onAction).not.toHaveBeenCalled();
});

it('keeps owner-only recovery controls out of shared, stale, completed and unrelated failures', () => {
  const sandbox = { ...parkedRuntimeUpdateFixture(), owner: 'owner' };
  for (const context of [
    { sandbox, viewerId: 'another-owner' },
    { sandbox, viewerId: undefined },
    { sandbox, viewerId: 'owner', stale: true },
    { sandbox: { ...sandbox, status: 'retired' as const }, viewerId: 'owner' },
    {
      sandbox: {
        ...sandbox,
        runtimeUpdate: { ...sandbox.runtimeUpdate!, completedAt: sandbox.updatedAt },
      },
      viewerId: 'owner',
    },
    {
      sandbox: { ...sandbox, status: 'failed' as const, runtimeUpdate: undefined },
      viewerId: 'owner',
    },
  ])
    expect(
      agentActions('ready', permissions, context).some(
        (action) => action.label === 'Retry runtime update',
      ),
    ).toBe(false);
});

it('offers review for a failed Sandbox with a failed update and preserves the busy gate', () => {
  const sandbox = { ...parkedRuntimeUpdateFixture(), owner: 'owner', status: 'failed' as const };
  const fresh = agentActions('failed', permissions, { sandbox, viewerId: 'owner' });
  expect(fresh.find((action) => action.label === 'Retry runtime update')).toMatchObject({
    route: { kind: 'sandbox', sandboxId: sandbox.id },
  });
  const busy = agentActions('failed', permissions, { sandbox, viewerId: 'owner', busy: true });
  expect(busy.find((action) => action.label === 'Retry runtime update')).toMatchObject({
    disabled: true,
    reason: 'Another action is still being confirmed.',
  });
});
