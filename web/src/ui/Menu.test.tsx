import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { Menu } from './Menu';
import { Presence } from './Presence';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

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

it('returns focus immediately and refuses actions while its exit frame fades', () => {
  vi.useFakeTimers();
  const select = vi.fn();
  const props = {
    anchor: <button>Options</button>,
    items: [{ id: 'pause', label: 'Pause' }],
    onSelect: select,
    onClose: vi.fn(),
  };
  const { rerender } = render(<Menu {...props} open />);
  const item = screen.getByRole('menuitem', { name: 'Pause' });
  const menu = screen.getByRole('menu');
  expect(item).toHaveFocus();
  rerender(<Menu {...props} open={false} />);
  expect(screen.getByRole('button', { name: 'Options' })).toHaveFocus();
  expect(screen.queryByRole('menu')).toBeNull();
  expect(menu).toHaveAttribute('inert');
  expect(menu).toHaveAttribute('aria-hidden', 'true');
  fireEvent.click(item);
  expect(select).not.toHaveBeenCalled();
  act(() => vi.advanceTimersByTime(200));
  expect(menu).not.toBeInTheDocument();
});

it('reopens the same menu without a stale exit removing it or resetting placement', () => {
  vi.useFakeTimers();
  const props = {
    anchor: <button>Options</button>,
    items: [{ id: 'pause', label: 'Pause' }],
    onSelect: vi.fn(),
    onClose: vi.fn(),
  };
  const { rerender } = render(<Menu {...props} open />);
  const menu = screen.getByRole('menu');
  rerender(<Menu {...props} open={false} />);
  act(() => vi.advanceTimersByTime(100));
  rerender(<Menu {...props} open />);
  act(() => vi.advanceTimersByTime(200));
  expect(screen.getAllByRole('menu')).toEqual([menu]);
  expect(menu).not.toHaveAttribute('inert');
  expect(menu).toHaveStyle({ visibility: 'visible' });
  fireEvent.click(screen.getByRole('menuitem', { name: 'Pause' }));
  expect(props.onSelect).toHaveBeenCalledExactlyOnceWith('pause');
});

it('removes the menu immediately when reduced motion is requested', () => {
  vi.spyOn(window, 'matchMedia').mockReturnValue({
    matches: true,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  } as unknown as MediaQueryList);
  const props = {
    anchor: <button>Options</button>,
    items: [{ id: 'pause', label: 'Pause' }],
    onSelect: vi.fn(),
    onClose: vi.fn(),
  };
  const { rerender } = render(<Menu {...props} open />);
  const menu = screen.getByRole('menu');
  rerender(<Menu {...props} open={false} />);
  expect(menu).not.toBeInTheDocument();
});

it('makes a portalled menu inert when its enclosing surface begins to exit', () => {
  vi.useFakeTimers();
  const select = vi.fn();
  const content = (
    <Menu
      open
      anchor={<button>Options</button>}
      items={[{ id: 'pause', label: 'Pause' }]}
      onSelect={select}
      onClose={vi.fn()}
    />
  );
  const { rerender } = render(<Presence>{content}</Presence>);
  const item = screen.getByRole('menuitem', { name: 'Pause' });
  rerender(<Presence>{null}</Presence>);
  expect(screen.queryByRole('menu')).toBeNull();
  fireEvent.click(item);
  expect(select).not.toHaveBeenCalled();
  act(() => vi.advanceTimersByTime(200));
  expect(document.querySelector('.menu')).toBeNull();
});

it('draws a navigation row as a real link that leaves modified clicks to the browser', async () => {
  const onSelect = vi.fn();
  const user = userEvent.setup();
  render(
    <Menu
      open
      anchor={<button>Services</button>}
      selectedId="here"
      items={[
        { id: 'here', label: 'Here', href: '/here' },
        { id: 'there', label: 'There', href: '/there' },
        { id: 'later', label: 'Later', href: '/later', disabled: true, reason: 'Not yet.' },
      ]}
      onSelect={onSelect}
      onClose={vi.fn()}
    />,
  );
  const there = screen.getByRole('menuitemradio', { name: 'There' });
  expect(there.tagName).toBe('A');
  expect(there).toHaveAttribute('href', '/there');
  expect(screen.getByRole('menuitemradio', { name: 'Here' })).toHaveAttribute(
    'aria-checked',
    'true',
  );
  // A disabled row is never a link, so it cannot be followed by any click.
  const later = screen.getByRole('menuitemradio', { name: 'Later Not yet.' });
  expect(later.tagName).toBe('BUTTON');
  expect(later).not.toHaveAttribute('href');

  const modified = new MouseEvent('click', { bubbles: true, cancelable: true, ctrlKey: true });
  there.dispatchEvent(modified);
  expect(modified.defaultPrevented).toBe(false);
  expect(onSelect).not.toHaveBeenCalled();

  // Arrow keys reach link rows as they reach buttons.
  expect(screen.getByRole('menuitemradio', { name: 'Here' })).toHaveFocus();
  await user.keyboard('{ArrowDown}');
  expect(there).toHaveFocus();
  await user.keyboard('{Enter}');
  expect(onSelect).toHaveBeenCalledWith('there');
});
