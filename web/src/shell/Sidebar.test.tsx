import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { Sidebar } from './Sidebar';
import { IconGoalOutline16 } from '../ui/icons';

it.each([true, false])(
  'retains the home link, service menu and account across collapse with creation %s',
  (creates) => {
    const draw = (collapsed: boolean, status: string) => (
      <Sidebar
        collapsed={collapsed}
        onToggleCollapsed={vi.fn()}
        account={<span>Account</span>}
        navigation={<span>{status}</span>}
        newAgent={creates ? { onCreate: vi.fn() } : undefined}
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
      expect(screen.getByRole('button', { name: 'Service: Agent Manager' })).toBeInTheDocument();
      expect(screen.getByText('Account')).toBeInTheDocument();
      expect(screen.queryByRole('button', { name: 'New agent' }) !== null).toBe(creates);
    }
    expect(screen.getByText('Active')).toBeInTheDocument();
  },
);

it('keeps New agent separate from sidebar collapse', async () => {
  const onCreate = vi.fn();
  const onToggleCollapsed = vi.fn();
  const actor = userEvent.setup();
  render(
    <Sidebar
      collapsed={false}
      onToggleCollapsed={onToggleCollapsed}
      account={<span>Account</span>}
      newAgent={{ onCreate }}
    />,
  );
  await actor.click(screen.getByRole('button', { name: 'New agent' }));
  expect(onCreate).toHaveBeenCalledOnce();
  expect(onToggleCollapsed).not.toHaveBeenCalled();
  await actor.click(screen.getByRole('button', { name: 'Hide navigation' }));
  expect(onToggleCollapsed).toHaveBeenCalledOnce();
});

it('keeps an unavailable New agent visible, disabled and explained', async () => {
  const onCreate = vi.fn();
  const actor = userEvent.setup();
  render(
    <Sidebar
      collapsed={false}
      onToggleCollapsed={vi.fn()}
      account={<span>Account</span>}
      newAgent={{ onCreate, unavailableReason: 'Switch to Personal to create an Agent.' }}
    />,
  );
  const button = screen.getByRole('button', { name: 'New agent' });
  expect(button).toHaveAttribute('aria-disabled', 'true');
  expect(button).toHaveAccessibleDescription('Switch to Personal to create an Agent.');
  expect(screen.getByText('Switch to Personal to create an Agent.')).toBeVisible();
  await actor.click(button);
  expect(onCreate).not.toHaveBeenCalled();
});

it('opens search and follows navigation links without reloading', async () => {
  const onSearch = vi.fn();
  const onNavigate = vi.fn();
  const actor = userEvent.setup();
  render(
    <Sidebar
      collapsed={false}
      onToggleCollapsed={vi.fn()}
      account={<span>Account</span>}
      onSearch={onSearch}
      onNavigate={onNavigate}
      links={[
        {
          id: 'triggers',
          label: 'Triggers',
          icon: <IconGoalOutline16 />,
          route: { kind: 'triggers' },
          current: true,
        },
      ]}
    />,
  );
  await actor.click(screen.getByRole('button', { name: 'Search' }));
  expect(onSearch).toHaveBeenCalledOnce();
  const triggers = screen.getByRole('link', { name: 'Triggers' });
  expect(triggers).toHaveAttribute('href', '/triggers/manage');
  expect(triggers).toHaveAttribute('aria-current', 'page');
  await actor.click(triggers);
  expect(onNavigate).toHaveBeenCalledWith({ kind: 'triggers' });
});

it('leaves a modified click on a navigation link to the browser', () => {
  const onNavigate = vi.fn();
  render(
    <Sidebar
      collapsed={false}
      onToggleCollapsed={vi.fn()}
      account={<span>Account</span>}
      onNavigate={onNavigate}
      links={[
        {
          id: 'triggers',
          label: 'Triggers',
          icon: <IconGoalOutline16 />,
          route: { kind: 'triggers' },
          current: false,
        },
      ]}
    />,
  );
  const link = screen.getByRole('link', { name: 'Triggers' });
  const event = new MouseEvent('click', { bubbles: true, cancelable: true, metaKey: true });
  link.dispatchEvent(event);
  expect(event.defaultPrevented).toBe(false);
  expect(onNavigate).not.toHaveBeenCalled();
  expect(link).not.toHaveAttribute('aria-current');
});

it('keeps navigation mounted across collapse', async () => {
  const draw = (collapsed: boolean) => (
    <Sidebar
      collapsed={collapsed}
      onToggleCollapsed={vi.fn()}
      account={<span>Account</span>}
      navigation={<input aria-label="Navigation draft" defaultValue="retained" />}
    />
  );
  const actor = userEvent.setup();
  const { rerender } = render(draw(false));
  const draft = screen.getByRole('textbox', { name: 'Navigation draft' });
  await actor.type(draft, ' state');
  rerender(draw(true));
  expect(draft).toBeInTheDocument();
  expect(draft).not.toBeVisible();
  rerender(draw(false));
  expect(screen.getByRole('textbox', { name: 'Navigation draft' })).toBe(draft);
  expect(draft).toHaveValue('retained state');
});
