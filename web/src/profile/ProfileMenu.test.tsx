import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { ProfileMenu, initialsFor } from './ProfileMenu';

const user = {
  id: 'subject-a',
  handle: 'ada',
  displayName: 'Ada Lovelace',
  email: 'ada@example.test',
  capabilities: [],
} as const;

it('keeps appearance and sign out in the account menu', async () => {
  const onSignOut = vi.fn();
  const actor = userEvent.setup();
  render(<ProfileMenu user={user} onSignOut={onSignOut} />);
  await actor.click(screen.getByRole('button', { name: /account menu/i }));
  expect(screen.getByRole('menuitem', { name: /appearance/i })).toBeInTheDocument();
  await actor.click(screen.getByRole('menuitem', { name: 'Sign out' }));
  expect(onSignOut).toHaveBeenCalledOnce();
});

it('derives stable initials from the display name', () => {
  expect(initialsFor(user)).toBe('AL');
});
