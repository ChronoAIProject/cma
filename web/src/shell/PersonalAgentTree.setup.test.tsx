import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { navigationFixture } from '../test/hierarchy';
import { Tree, type TreeProps } from './Tree';

function draw(
  overrides: Partial<TreeProps> = {},
  status: 'provisioning' | 'installing' = 'installing',
) {
  return (
    <Tree
      viewerId="owner"
      view="agent"
      items={navigationFixture(status, 'starting').map((item) => ({
        ...item,
        sandbox: { ...item.sandbox, owner: 'owner' },
      }))}
      route={{ kind: 'agent', agentId: 'agt_1' }}
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

it.each(['provisioning', 'installing'] as const)(
  'offers the verified %s parent without dispatching an Agent command',
  (status) => {
    const onNavigate = vi.fn(),
      onAction = vi.fn();
    render(draw({ onNavigate, onAction }, status));
    fireEvent.click(screen.getByRole('button', { name: 'Options for Agent one' }));
    const review = screen.getByRole('menuitem', { name: /^Review environment/ });
    expect(review).toHaveTextContent('all Agents inside it');
    fireEvent.click(review);
    expect(onNavigate).toHaveBeenCalledExactlyOnceWith({ kind: 'sandbox', sandboxId: 'sbx_1' });
    expect(onAction).not.toHaveBeenCalled();
  },
);

it.each([{ viewerId: 'other-owner' }, { refreshError: 'Checking status' }])(
  'omits the setup shortcut for unverified ownership or freshness: %j',
  (overrides) => {
    render(draw(overrides));
    fireEvent.click(screen.getByRole('button', { name: 'Options for Agent one' }));
    expect(screen.queryByRole('menuitem', { name: /^Review environment/ })).not.toBeInTheDocument();
  },
);

it('does not offer a parent whose identity differs from the Agent summary', () => {
  const items = navigationFixture('installing', 'starting').map((item) => ({
    ...item,
    sandbox: { ...item.sandbox, id: 'sbx_other', owner: 'owner' },
  }));
  const onNavigate = vi.fn(),
    onAction = vi.fn();
  render(draw({ items, onNavigate, onAction }));
  fireEvent.click(screen.getByRole('button', { name: 'Options for Agent one' }));
  expect(screen.queryByRole('menuitem', { name: /^Review environment/ })).not.toBeInTheDocument();
  expect(onNavigate).not.toHaveBeenCalled();
  expect(onAction).not.toHaveBeenCalled();
});
