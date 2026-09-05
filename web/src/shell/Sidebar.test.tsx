import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { Sidebar } from './Sidebar';

it('uses the owner brand row as the New agent shortcut', async () => {
  const onNewAgent = vi.fn();
  const actor = userEvent.setup();
  const { rerender } = render(
    <Sidebar
      collapsed={false}
      onToggleCollapsed={vi.fn()}
      account={<span>Account</span>}
      canCreateAgent
      onNewAgent={onNewAgent}
    />,
  );
  await actor.click(screen.getByRole('button', { name: 'New agent' }));
  expect(onNewAgent).toHaveBeenCalledOnce();

  rerender(
    <Sidebar collapsed={false} onToggleCollapsed={vi.fn()} account={<span>Account</span>} />,
  );
  expect(screen.getByRole('link', { name: 'CMA home' })).toHaveAttribute('href', '/');
});
