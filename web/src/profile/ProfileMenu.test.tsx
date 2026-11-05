import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { BUILD_INFO } from '../shell/build';
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
  it('keeps Docs, appearance and sign out without service or developer actions', async () => {
    const onSignOut = vi.fn();
    const onNavigate = vi.fn();
    const actor = userEvent.setup();
    render(<ProfileMenu user={user} onSignOut={onSignOut} onNavigate={onNavigate} />);
    await actor.click(screen.getByRole('button', { name: /account menu/i }));
    for (const name of ['Event Gateway consumers', 'Developer']) {
      expect(screen.queryByRole('menuitem', { name })).not.toBeInTheDocument();
    }
    expect(screen.queryByRole('menuitem', { name: 'Marketplace' })).not.toBeInTheDocument();
    expect(screen.queryByRole('menuitem', { name: 'Operations' })).not.toBeInTheDocument();
    expect(screen.queryByRole('menuitem', { name: 'CMA Trigger' })).not.toBeInTheDocument();
    expect(screen.getByRole('menuitem', { name: /appearance/i })).toBeInTheDocument();

    await actor.click(screen.getByRole('menuitem', { name: 'Docs' }));
    expect(onNavigate).toHaveBeenCalledWith({ kind: 'docs' });
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
});

it('derives stable initials from the display name', () => {
  expect(initialsFor(user)).toBe('AL');
});

it('provides docs and explicit text sizes without the relocated Agent/trigger actions', async () => {
  const actor = userEvent.setup();
  const navigate = vi.fn();
  render(
    <ProfileMenu
      user={{
        ...user,
        capabilities: ['use_own_sandboxes', 'publish_triggers', 'read_activity_log'],
      }}
      onNavigate={navigate}
      onSignOut={vi.fn()}
    />,
  );
  await actor.click(screen.getByRole('button', { name: /account menu/i }));
  expect(screen.queryByRole('menuitem', { name: 'Agents' })).not.toBeInTheDocument();
  expect(screen.queryByRole('menuitem', { name: 'CMA Trigger' })).not.toBeInTheDocument();
  const menu = screen.getByRole('menu');
  const docs = within(menu).getByRole('menuitem', { name: 'Docs' });
  const ops = within(menu).getByRole('menuitem', { name: 'Operations' });
  expect(docs.compareDocumentPosition(ops) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  const sizes = screen.getByRole('group', { name: 'System text size' });
  await actor.click(within(sizes).getByRole('menuitemradio', { name: 'Large' }));
  expect(within(sizes).getByRole('menuitemradio', { name: 'Large' })).toHaveAttribute(
    'aria-checked',
    'true',
  );
  expect(document.documentElement.dataset.systemTextSize).toBe('large');
  expect(localStorage.getItem('cma:system-text-size')).toBe('large');
  await actor.click(docs);
  expect(navigate).toHaveBeenCalledWith({ kind: 'docs' });
});

it('keeps text preferences in sync between retained portal account menus', async () => {
  const actor = userEvent.setup();
  const one = render(<ProfileMenu user={user} onSignOut={vi.fn()} />);
  const two = render(
    <ProfileMenu user={{ ...user, displayName: 'Second surface' }} onSignOut={vi.fn()} />,
  );
  await actor.click(within(one.container).getByRole('button'));
  await actor.click(screen.getByRole('menuitemradio', { name: 'Small' }));
  await actor.click(within(two.container).getByRole('button'));
  expect(screen.getByRole('menuitemradio', { name: 'Small' })).toHaveAttribute(
    'aria-checked',
    'true',
  );
  await actor.click(screen.getByRole('menuitemradio', { name: 'Medium' }));
});

it('copies the immutable build identity from the account menu', async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  // After setup, which installs its own clipboard on the navigator.
  const actor = userEvent.setup();
  Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
  render(<ProfileMenu user={user} onSignOut={vi.fn()} onNavigate={vi.fn()} />);
  await actor.click(screen.getByRole('button', { name: /account menu/i }));
  await actor.click(screen.getByRole('menuitem', { name: /Copy build version · dev \(local\)/ }));
  expect(writeText).toHaveBeenCalledWith(BUILD_INFO.copyText);
  expect(await screen.findByRole('menuitem', { name: 'Build version copied' })).toBeInTheDocument();
  expect(screen.getByRole('status')).toHaveTextContent('Build version copied');
  expect(screen.getByRole('menu')).not.toContainElement(screen.getByRole('status'));
});

it('keeps the build revision readable when the clipboard refuses it', async () => {
  const writeText = vi.fn().mockRejectedValue(new Error('denied'));
  // After setup, which installs its own clipboard on the navigator.
  const actor = userEvent.setup();
  Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
  render(<ProfileMenu user={user} onSignOut={vi.fn()} onNavigate={vi.fn()} />);
  await actor.click(screen.getByRole('button', { name: /account menu/i }));
  await actor.click(screen.getByRole('menuitem', { name: /Copy build version · dev \(local\)/ }));
  expect(writeText).toHaveBeenCalledOnce();
  expect(screen.getByRole('status')).toHaveTextContent('Could not copy');
  expect(screen.getByText(BUILD_INFO.copyText)).toBeInTheDocument();
  expect(window.getSelection()?.toString()).toBe(BUILD_INFO.copyText);
  writeText.mockResolvedValue(undefined);
  await actor.click(screen.getByRole('menuitem', { name: /Copy build version/ }));
  expect(screen.getByRole('status')).toHaveTextContent('Build version copied');
});

it('provides a selectable identity when the Clipboard API is absent', async () => {
  const actor = userEvent.setup();
  Object.defineProperty(navigator, 'clipboard', { value: undefined, configurable: true });
  render(<ProfileMenu user={user} onSignOut={vi.fn()} />);
  await actor.click(screen.getByRole('button', { name: /account menu/i }));
  await actor.click(screen.getByRole('menuitem', { name: /Copy build version/ }));
  expect(screen.getByRole('status')).toHaveTextContent('Could not copy');
  expect(screen.getByText(BUILD_INFO.copyText)).toBeInTheDocument();
  expect(screen.getByRole('menuitem', { name: /Copy build version/ })).toBeEnabled();
});
