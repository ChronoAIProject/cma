import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { ProfileMenu, initialsFor } from './ProfileMenu';

const user = {
  id: 'subject-a',
  handle: 'ada',
  displayName: 'Ada Lovelace',
  email: 'ada@example.test',
  capabilities: [],
  visitor: false,
} as const;

describe('account menu', () => {
  it('navigates to daily product surfaces and keeps appearance and sign out', async () => {
    const onSignOut = vi.fn();
    const onNavigate = vi.fn();
    const actor = userEvent.setup();
    render(<ProfileMenu user={user} bots onSignOut={onSignOut} onNavigate={onNavigate} />);
    await actor.click(screen.getByRole('button', { name: /account menu/i }));
    for (const name of ['Profiles', 'Bot settings', 'Developer']) {
      expect(screen.getByRole('menuitem', { name })).toBeInTheDocument();
    }
    expect(screen.queryByRole('menuitem', { name: 'Marketplace' })).not.toBeInTheDocument();
    expect(screen.queryByRole('menuitem', { name: 'Operations' })).not.toBeInTheDocument();
    expect(screen.queryByRole('menuitem', { name: 'CMA Trigger' })).not.toBeInTheDocument();
    expect(screen.getByRole('menuitem', { name: /appearance/i })).toBeInTheDocument();

    await actor.click(screen.getByRole('menuitem', { name: 'Profiles' }));
    expect(onNavigate).toHaveBeenCalledWith({ kind: 'profiles' });
    await actor.click(screen.getByRole('button', { name: /account menu/i }));
    await actor.click(screen.getByRole('menuitem', { name: 'Bot settings' }));
    expect(onNavigate).toHaveBeenCalledWith({ kind: 'bots' });
    await actor.click(screen.getByRole('button', { name: /account menu/i }));
    await actor.click(screen.getByRole('menuitem', { name: 'Sign out' }));
    expect(onSignOut).toHaveBeenCalledOnce();
  });

  it('shows Operations only with an operator capability', async () => {
    const actor = userEvent.setup();
    render(
      <ProfileMenu
        user={{ ...user, capabilities: ['read_activity_log'] }}
        onSignOut={vi.fn()}
        onNavigate={vi.fn()}
      />,
    );
    await actor.click(screen.getByRole('button', { name: /account menu/i }));
    expect(screen.getByRole('menuitem', { name: 'Operations' })).toBeInTheDocument();
  });

  it('keeps Event Gateway consumers in the owner account menu and routes there', async () => {
    const onNavigate = vi.fn();
    const actor = userEvent.setup();
    render(
      <ProfileMenu
        user={{ ...user, capabilities: ['use_own_sandboxes'] }}
        onSignOut={vi.fn()}
        onNavigate={onNavigate}
        canManageBridges
      />,
    );
    await actor.click(screen.getByRole('button', { name: /account menu/i }));
    await actor.click(screen.getByRole('menuitem', { name: 'Event Gateway consumers' }));
    expect(onNavigate).toHaveBeenCalledWith({ kind: 'event_gateway' });
  });

  it('does not expose Event Gateway consumers to visitors', async () => {
    const actor = userEvent.setup();
    render(
      <ProfileMenu
        user={{ ...user, visitor: true }}
        onSignOut={vi.fn()}
        onNavigate={vi.fn()}
        canManageBridges
      />,
    );
    await actor.click(screen.getByRole('button', { name: /account menu/i }));
    expect(screen.queryByRole('menuitem', { name: 'Event Gateway consumers' })).not.toBeInTheDocument();
  });
});

it('derives stable initials from the display name', () => {
  expect(initialsFor(user)).toBe('AL');
});
