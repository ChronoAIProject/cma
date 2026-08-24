import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { Console } from '../Console';
import type { User } from '../auth/types';

const USER: User = {
  id: 'sub_developer',
  handle: 'developer',
  capabilities: ['create_session', 'use_own_sessions'],
};

async function openAccount(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole('button', { name: /account menu/i }));
}

describe('Developer as a Console surface', () => {
  it('is available to every signed-in user and swaps the centre without routing', async () => {
    const user = userEvent.setup();
    const address = window.location.href;
    const { container } = render(<Console user={USER} onSignOut={() => {}} />);
    await openAccount(user);

    await user.click(screen.getByRole('menuitem', { name: 'Developer' }));

    expect(await screen.findByRole('region', { name: 'Developer' })).toBeInTheDocument();
    expect(container.querySelector('.console__surface')).toBeNull();
    expect(container.querySelector('.modal__mask')).toBeNull();
    expect(container.querySelectorAll('.frame')).toHaveLength(1);
    expect(container.querySelectorAll('.sidebar')).toHaveLength(1);
    expect(window.location.href).toBe(address);
  });

  it('names the return action and returns to sessions', async () => {
    const user = userEvent.setup();
    render(<Console user={USER} onSignOut={() => {}} />);
    await openAccount(user);
    await user.click(screen.getByRole('menuitem', { name: 'Developer' }));
    await screen.findByRole('region', { name: 'Developer' });

    await openAccount(user);
    await user.click(screen.getByRole('menuitem', { name: 'Back to sessions' }));

    await waitFor(() => {
      expect(screen.queryByRole('region', { name: 'Developer' })).toBeNull();
    });
    expect(document.querySelector('.console__surface')).not.toBeNull();
  });
});
