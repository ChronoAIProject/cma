import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { Menu } from './Menu';

it('keeps an unavailable action and its reason keyboard reachable without admitting the command', async () => {
  const onSelect = vi.fn();
  const user = userEvent.setup();
  render(
    <Menu
      open
      anchor={<button>Options</button>}
      items={[
        { id: 'pause', label: 'Pause', disabled: true, reason: 'Progress is being saved.' },
        { id: 'other', label: 'Another action' },
      ]}
      onSelect={onSelect}
      onClose={vi.fn()}
    />,
  );
  const pause = screen.getByRole('menuitem', { name: 'Pause Progress is being saved.' });
  expect(pause).toHaveFocus();
  expect(pause).toHaveAttribute('aria-disabled', 'true');
  expect(pause).toHaveAttribute('data-has-reason');
  await user.keyboard('{Enter}');
  expect(onSelect).not.toHaveBeenCalled();
  await user.keyboard('{ArrowDown}{Enter}');
  expect(onSelect).toHaveBeenCalledWith('other');
});
