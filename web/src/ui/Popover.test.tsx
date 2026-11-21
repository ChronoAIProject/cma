import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { expect, it } from 'vitest';
import { Menu } from './Menu';
import { Popover } from './Popover';

function Harness() {
  const [open, setOpen] = useState(false);
  const [listOpen, setListOpen] = useState(false);
  return (
    <>
      <Popover
        open={open}
        label="Card"
        onClose={() => setOpen(false)}
        anchor={
          <button type="button" aria-expanded={open} onClick={() => setOpen((was) => !was)}>
            Open card
          </button>
        }
      >
        <button type="button">First control</button>
        <Menu
          open={listOpen}
          label="Nested"
          items={[{ id: 'one', label: 'One' }]}
          onClose={() => setListOpen(false)}
          onSelect={() => setListOpen(false)}
          anchor={
            <button type="button" onClick={() => setListOpen((was) => !was)}>
              Open list
            </button>
          }
        />
      </Popover>
      <button type="button">Elsewhere</button>
    </>
  );
}

it('draws the card on the body, focuses its first control and returns focus on Escape', async () => {
  const user = userEvent.setup();
  render(<Harness />);
  await user.click(screen.getByRole('button', { name: 'Open card' }));
  const card = screen.getByRole('dialog', { name: 'Card' });
  expect(card.parentElement).toBe(document.body);
  expect(screen.getByRole('button', { name: 'First control' })).toHaveFocus();
  await user.keyboard('{Escape}');
  expect(screen.queryByRole('dialog', { name: 'Card' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Open card' })).toHaveFocus();
});

it('treats a nested portalled list as part of the card for presses and focus', async () => {
  const user = userEvent.setup();
  render(<Harness />);
  await user.click(screen.getByRole('button', { name: 'Open card' }));
  await user.click(screen.getByRole('button', { name: 'Open list' }));
  // The list's row takes focus; it is outside the card's DOM but inside its tree.
  const row = screen.getByRole('menuitem', { name: 'One' });
  expect(row).toHaveFocus();
  await act(() => new Promise((resolve) => setTimeout(resolve, 10)));
  expect(screen.getByRole('dialog', { name: 'Card' })).toBeInTheDocument();
  await user.click(row);
  expect(screen.queryByRole('menu', { name: 'Nested' })).not.toBeInTheDocument();
  expect(screen.getByRole('dialog', { name: 'Card' })).toBeInTheDocument();
});

it('closes when focus or a press leaves the card for anywhere else', async () => {
  const user = userEvent.setup();
  render(<Harness />);
  await user.click(screen.getByRole('button', { name: 'Open card' }));
  screen.getByRole('button', { name: 'Elsewhere' }).focus();
  await act(() => new Promise((resolve) => setTimeout(resolve, 10)));
  expect(screen.queryByRole('dialog', { name: 'Card' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Open card' }));
  expect(screen.getByRole('dialog', { name: 'Card' })).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Open card' }));
  expect(screen.queryByRole('dialog', { name: 'Card' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Open card' }));
  await user.click(document.body);
  expect(screen.queryByRole('dialog', { name: 'Card' })).not.toBeInTheDocument();
});
