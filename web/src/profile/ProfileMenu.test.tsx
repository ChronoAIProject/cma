import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ProfileMenu } from './ProfileMenu';
import type { User } from '../auth/types';

const USER: User = { id: 'subject-1', handle: 'operator', capabilities: [] };

describe('ProfileMenu', () => {
  it('stays closed until asked', () => {
    render(<ProfileMenu user={USER} onSignOut={vi.fn()} />);
    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('opens and moves focus into the menu', async () => {
    const user = userEvent.setup();
    render(<ProfileMenu user={USER} onSignOut={vi.fn()} />);

    await user.click(screen.getByRole('button', { name: /account menu/i }));

    // A keyboard user left behind on the trigger has no signal anything opened.
    expect(screen.getByRole('menuitem', { name: /sign out/i })).toHaveFocus();
  });

  it('signs out', async () => {
    const user = userEvent.setup();
    const onSignOut = vi.fn();
    render(<ProfileMenu user={USER} onSignOut={onSignOut} />);

    await user.click(screen.getByRole('button', { name: /account menu/i }));
    await user.click(screen.getByRole('menuitem', { name: /sign out/i }));

    expect(onSignOut).toHaveBeenCalledOnce();
    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('closes on Escape and returns focus to the trigger', async () => {
    const user = userEvent.setup();
    render(<ProfileMenu user={USER} onSignOut={vi.fn()} />);
    const trigger = screen.getByRole('button', { name: /account menu/i });

    await user.click(trigger);
    await user.keyboard('{Escape}');

    expect(screen.queryByRole('menu')).toBeNull();
    // Otherwise the user is dropped at the top of the document.
    expect(trigger).toHaveFocus();
  });

  it('closes on an outside click', async () => {
    const user = userEvent.setup();
    render(
      <div>
        <ProfileMenu user={USER} onSignOut={vi.fn()} />
        <button type="button">elsewhere</button>
      </div>,
    );

    await user.click(screen.getByRole('button', { name: /account menu/i }));
    await user.click(screen.getByRole('button', { name: 'elsewhere' }));

    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('is operable from the keyboard alone', async () => {
    const user = userEvent.setup();
    const onSignOut = vi.fn();
    render(<ProfileMenu user={USER} onSignOut={onSignOut} />);

    await user.tab();
    await user.keyboard('{Enter}');
    await user.keyboard('{Enter}');

    expect(onSignOut).toHaveBeenCalledOnce();
  });

  it('reports its expanded state to assistive technology', async () => {
    const user = userEvent.setup();
    render(<ProfileMenu user={USER} onSignOut={vi.fn()} />);
    const trigger = screen.getByRole('button', { name: /account menu/i });

    expect(trigger).toHaveAttribute('aria-expanded', 'false');
    await user.click(trigger);
    expect(trigger).toHaveAttribute('aria-expanded', 'true');
  });

  it('falls back to initials when there is no avatar', () => {
    render(
      <ProfileMenu
        user={{ id: 'x', handle: 'ada.lovelace', capabilities: [] }}
        onSignOut={vi.fn()}
      />,
    );
    expect(screen.getByRole('button', { name: /account menu/i }).textContent).toContain('AL');
  });

  it('renders an avatar when one is supplied', () => {
    render(
      <ProfileMenu
        user={{ id: 'x', handle: 'ada', avatarUrl: 'https://example.test/a.png', capabilities: [] }}
        onSignOut={vi.fn()}
      />,
    );
    // Decorative: the accessible name already comes from the trigger's label.
    expect(
      screen.getByRole('button', { name: /account menu/i }).querySelector('img'),
    ).toHaveAttribute('alt', '');
  });

  it('survives a blank handle rather than rendering an empty control', () => {
    render(<ProfileMenu user={{ id: 'x', handle: '   ', capabilities: [] }} onSignOut={vi.fn()} />);
    expect(screen.getByRole('button', { name: /account menu/i }).textContent).toContain('?');
  });
});

describe('the operator entry point', () => {
  it('is not in the account menu at all', async () => {
    // §14 makes Operations a page and one of §1's two views, so it is reached
    // by the app bar's switch rather than by an account action. It was here
    // when it was a dialog; a menu item beside it would be a second route to
    // a view, which is how a page frame starts becoming navigation.
    const user = userEvent.setup();
    render(<ProfileMenu user={USER} onSignOut={vi.fn()} />);

    await user.click(screen.getByRole('button', { name: /account menu/i }));
    expect(screen.queryByRole('menuitem', { name: /operations/i })).toBeNull();
  });
});
