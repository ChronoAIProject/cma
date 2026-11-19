import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { parkedRuntimeUpdateFixture, workspaceFixture } from '../test/hierarchy';
import { Tree, type TreePermissions } from './Tree';

const permissions: TreePermissions = {
  createWorkspace: true,
  createAgent: true,
  pauseSandbox: true,
  deleteSandbox: true,
  removeWorkspace: true,
  stopAgent: true,
  deleteAgent: true,
  exportHistory: true,
};

it.each([false, true])(
  'offers Delete for a parked update with retained request=%s',
  async (retained) => {
    const sandbox = parkedRuntimeUpdateFixture();
    const onAction = vi.fn();
    render(
      <Tree
        viewerId="owner"
        view="sandbox"
        items={[{ sandbox, workspaces: [workspaceFixture()] }]}
        busySandboxIds={new Set(retained ? [sandbox.id] : [])}
        route={{ kind: 'home' }}
        permissions={permissions}
        onNavigate={vi.fn()}
        onAction={onAction}
      />,
    );
    const actor = userEvent.setup();
    await actor.click(screen.getByRole('button', { name: 'Options for Atlas' }));
    const remove = screen.getByRole('menuitem', { name: /^Delete/ });
    expect(remove).not.toHaveAttribute('aria-disabled', 'true');
    await actor.click(remove);
    expect(onAction).toHaveBeenCalledExactlyOnceWith({
      kind: 'delete_sandbox',
      sandboxId: sandbox.id,
    });
  },
);

it.each(['stale', 'submitting', 'checkpoint', 'updating', 'no-permission'] as const)(
  'keeps the tree deletion gate for %s',
  async (condition) => {
    const parked = parkedRuntimeUpdateFixture();
    const sandbox = {
      ...parked,
      lifecycle: {
        ...parked.lifecycle!,
        checkpoint:
          condition === 'checkpoint'
            ? { phase: 'capture' as const, epoch: 1, startedAt: '2026-11-19T00:00:00Z' }
            : undefined,
      },
      runtimeUpdate: {
        ...parked.runtimeUpdate!,
        state: condition === 'updating' ? ('updating' as const) : ('failed' as const),
      },
    };
    const onAction = vi.fn();
    render(
      <Tree
        viewerId="owner"
        view="sandbox"
        items={[{ sandbox, workspaces: [workspaceFixture()] }]}
        lifecycleBusy={condition === 'submitting'}
        busySandboxIds={new Set([sandbox.id])}
        refreshError={condition === 'stale' ? 'Status is unavailable.' : undefined}
        route={{ kind: 'home' }}
        permissions={{ ...permissions, deleteSandbox: condition !== 'no-permission' }}
        onNavigate={vi.fn()}
        onAction={onAction}
      />,
    );
    const actor = userEvent.setup();
    await actor.click(screen.getByRole('button', { name: 'Options for Atlas' }));
    const remove = screen.queryByRole('menuitem', { name: /^Delete/ });
    if (condition === 'no-permission') expect(remove).not.toBeInTheDocument();
    else {
      expect(remove).toHaveAttribute('aria-disabled', 'true');
      await actor.click(remove!);
    }
    expect(onAction).not.toHaveBeenCalled();
  },
);
