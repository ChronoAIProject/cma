import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { Sidebar } from './Sidebar';

it.each([true, false])(
  'retains account and hierarchy controls without duplicating the global brand with creation %s',
  (canCreateAgent) => {
    const draw = (collapsed: boolean, status: string) => (
      <Sidebar
        collapsed={collapsed}
        onToggleCollapsed={vi.fn()}
        account={<span>Account</span>}
        navigation={<span>{status}</span>}
        canCreateAgent={canCreateAgent}
        onNewAgent={vi.fn()}
      />
    );
    const { rerender } = render(draw(false, 'Active'));
    for (const [collapsed, status] of [
      [false, 'Saving progress'],
      [true, 'Finishing save'],
      [false, 'Active'],
    ] as const) {
      rerender(draw(collapsed, status));
      expect(screen.queryByRole('link', { name: 'CMA home' })).not.toBeInTheDocument();
      expect(screen.queryByLabelText(/^Build /)).not.toBeInTheDocument();
      expect(screen.getByText('Account')).toBeInTheDocument();
    }
    expect(screen.getByText('Active')).toBeInTheDocument();
  },
);

it('keeps New agent separate from sidebar collapse and permission gating', async () => {
  const onNewAgent = vi.fn();
  const onToggleCollapsed = vi.fn();
  const actor = userEvent.setup();
  const { rerender } = render(
    <Sidebar
      collapsed={false}
      onToggleCollapsed={onToggleCollapsed}
      account={<span>Account</span>}
      canCreateAgent
      onNewAgent={onNewAgent}
    />,
  );
  await actor.click(screen.getByRole('button', { name: 'New agent' }));
  expect(onNewAgent).toHaveBeenCalledOnce();
  expect(onToggleCollapsed).not.toHaveBeenCalled();
  await actor.click(screen.getByRole('button', { name: 'Hide navigation' }));
  expect(onToggleCollapsed).toHaveBeenCalledOnce();

  rerender(
    <Sidebar
      collapsed={false}
      onToggleCollapsed={onToggleCollapsed}
      account={<span>Account</span>}
    />,
  );
  expect(screen.queryByRole('button', { name: 'New agent' })).not.toBeInTheDocument();
});
