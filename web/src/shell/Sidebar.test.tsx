import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { Sidebar } from './Sidebar';

it.each([true, false])(
  'retains account and hierarchy controls with the shared brand with creation %s',
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
      expect(screen.getByRole('link', { name: 'CMA home' })).toBeInTheDocument();
      expect(screen.getByLabelText(/^Build /)).toBeInTheDocument();
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

it('keeps navigation mounted across collapse and exposes separate trigger creation', async () => {
  const create = vi.fn();
  const draw = (collapsed: boolean) => (
    <Sidebar
      collapsed={collapsed}
      onToggleCollapsed={vi.fn()}
      account={<span>Account</span>}
      navigation={<input aria-label="Navigation draft" defaultValue="retained" />}
      canCreateTrigger
      onNewTrigger={create}
    />
  );
  const actor = userEvent.setup();
  const { rerender } = render(draw(false));
  const draft = screen.getByRole('textbox', { name: 'Navigation draft' });
  await actor.type(draft, ' state');
  await actor.click(screen.getByRole('button', { name: 'New agent trigger' }));
  expect(create).toHaveBeenCalledOnce();
  rerender(draw(true));
  expect(draft).toBeInTheDocument();
  expect(draft).not.toBeVisible();
  rerender(draw(false));
  expect(screen.getByRole('textbox', { name: 'Navigation draft' })).toBe(draft);
  expect(draft).toHaveValue('retained state');
});
