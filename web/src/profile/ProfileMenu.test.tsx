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

    /*
     * The first row, which is what the shared menu does everywhere. This used
     * to land on Sign out — the last item — because the hand-rolled version
     * carried its focus ref on whichever button happened to be written last.
     * Opening a menu onto its most destructive entry is not a decision anybody
     * made.
     */
    expect(screen.getByRole('menuitem', { name: /appearance/i })).toHaveFocus();
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
    // Focus opens on the first row, so Sign out is reached by walking to it.
    // Arrow keys are the shared menu's, and this is the account menu proving
    // it inherited them rather than each menu growing its own.
    await user.keyboard('{ArrowUp}');
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
  /*
   * Operations lives here now. It was the app bar's view switch until §9
   * retired the bar, and then a row of its own in the sidebar foot — which
   * made the foot a two-item navigation strip beside an account control that
   * omitted the one place an operator goes. It is still one of §1's three views
   * and still not navigation: one entry, which names where it goes.
   */
  it('is offered to a caller who may operate', async () => {
    const user = userEvent.setup();
    render(<ProfileMenu user={USER} onSignOut={vi.fn()} onOperations={vi.fn()} />);

    await user.click(screen.getByRole('button', { name: /account menu/i }));
    expect(screen.getByRole('menuitem', { name: 'Operations' })).toBeInTheDocument();
  });

  it('is absent, never disabled, for anybody else', async () => {
    // §12: an action that will never work is omitted rather than drawn dead.
    // Presentation, never protection — the control plane refuses on its own.
    const user = userEvent.setup();
    render(<ProfileMenu user={USER} onSignOut={vi.fn()} />);

    await user.click(screen.getByRole('button', { name: /account menu/i }));
    expect(screen.queryByRole('menuitem', { name: /operations/i })).toBeNull();
  });

  it('names where it goes when the panel is already showing', async () => {
    // Rather than marking itself current: a check would make it a value among
    // alternatives, and these rows are actions.
    const user = userEvent.setup();
    render(<ProfileMenu user={USER} onSignOut={vi.fn()} onOperations={vi.fn()} operationsActive />);

    await user.click(screen.getByRole('button', { name: /account menu/i }));
    expect(screen.getByRole('menuitem', { name: 'Back to sessions' })).toBeInTheDocument();
  });

  it('takes the caller there', async () => {
    const user = userEvent.setup();
    const onOperations = vi.fn();
    render(<ProfileMenu user={USER} onSignOut={vi.fn()} onOperations={onOperations} />);

    await user.click(screen.getByRole('button', { name: /account menu/i }));
    await user.click(screen.getByRole('menuitem', { name: 'Operations' }));
    expect(onOperations).toHaveBeenCalledOnce();
  });
});

describe('the Developer entry point', () => {
  it('is present when the surface is wired and remains an action', async () => {
    const user = userEvent.setup();
    render(<ProfileMenu user={USER} onSignOut={vi.fn()} onDeveloper={vi.fn()} />);

    await user.click(screen.getByRole('button', { name: /account menu/i }));
    expect(screen.getByRole('menuitem', { name: 'Developer' })).toBeInTheDocument();
    expect(screen.queryByRole('menuitemradio')).toBeNull();
  });

  it('names the return destination while Developer is active', async () => {
    const user = userEvent.setup();
    render(<ProfileMenu user={USER} onSignOut={vi.fn()} onDeveloper={vi.fn()} developerActive />);

    await user.click(screen.getByRole('button', { name: /account menu/i }));
    expect(screen.getByRole('menuitem', { name: 'Back to sessions' })).toBeInTheDocument();
  });

  it('opens the surface and closes the menu', async () => {
    const user = userEvent.setup();
    const onDeveloper = vi.fn();
    render(<ProfileMenu user={USER} onSignOut={vi.fn()} onDeveloper={onDeveloper} />);

    await user.click(screen.getByRole('button', { name: /account menu/i }));
    await user.click(screen.getByRole('menuitem', { name: 'Developer' }));

    expect(onDeveloper).toHaveBeenCalledOnce();
    expect(screen.queryByRole('menu')).toBeNull();
  });
});

describe('the terminal palette', () => {
  it('is not an account setting', async () => {
    /*
     * It sat here, applying to a plane most sessions are not on — an account
     * setting for a surface its owner may never open. It belongs to the
     * session that draws a terminal, in that session's own overflow menu.
     */
    const user = userEvent.setup();
    render(<ProfileMenu user={USER} onSignOut={vi.fn()} />);

    await user.click(screen.getByRole('button', { name: /account menu/i }));
    expect(screen.queryByRole('menuitem', { name: /terminal theme/i })).toBeNull();
  });
});
