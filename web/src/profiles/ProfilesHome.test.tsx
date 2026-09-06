import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { AGENT_PROFILE, profileClient } from '../test/profiles';
import { ProfilesHome } from './ProfilesHome';

it('lists own profiles by kind with revision, visibility, updated time and keyboard tabs', async () => {
  const actor = userEvent.setup();
  const listProfiles = vi.fn(profileClient().listProfiles);
  const onNavigate = vi.fn();
  const onNewProfile = vi.fn();
  render(
    <ProfilesHome client={{ listProfiles }} onNavigate={onNavigate} onNewProfile={onNewProfile} />,
  );
  expect(await screen.findByRole('link', { name: 'Research agent' })).toHaveAttribute(
    'href',
    '/profiles/sandbox/agp_research',
  );
  expect(screen.getByText('Revision 1')).toBeInTheDocument();
  expect(screen.getByText('Private')).toBeInTheDocument();
  screen.getByRole('tab', { name: 'Sandbox' }).focus();
  await actor.keyboard('{End}');
  expect(screen.getByRole('tab', { name: 'Agent' })).toHaveAttribute('aria-selected', 'true');
  await waitFor(() => expect(listProfiles).toHaveBeenCalledWith('agent', expect.any(Object)));
  await actor.click(await screen.findByRole('link', { name: 'Research agent' }));
  expect(onNavigate).toHaveBeenCalledWith({
    kind: 'profile',
    profileKind: 'agent',
    profileId: AGENT_PROFILE.id,
  });
  await actor.click(screen.getByRole('button', { name: 'New profile' }));
  expect(onNewProfile).toHaveBeenCalledWith('agent');
});

it('ignores a late response from the previous kind after a tab change', async () => {
  const actor = userEvent.setup();
  let settle: ((page: { items: (typeof AGENT_PROFILE)[] }) => void) | undefined;
  let firstSignal: AbortSignal | undefined;
  const listProfiles = vi
    .fn()
    .mockImplementationOnce((_kind, options) => {
      firstSignal = options.signal;
      return new Promise((resolve) => {
        settle = resolve;
      });
    })
    .mockResolvedValue({ items: [] });
  render(<ProfilesHome client={{ listProfiles }} onNavigate={vi.fn()} onNewProfile={vi.fn()} />);
  await actor.click(screen.getByRole('tab', { name: 'Agent' }));
  expect(await screen.findByText('No agent profiles yet.')).toBeInTheDocument();
  expect(firstSignal?.aborted).toBe(true);
  await act(async () => {
    settle?.({ items: [AGENT_PROFILE] });
  });
  expect(screen.queryByRole('link', { name: 'Research agent' })).not.toBeInTheDocument();
});

it('appends additional pages and clears the prior kind when changing tabs', async () => {
  const actor = userEvent.setup();
  const listProfiles = vi
    .fn()
    .mockResolvedValueOnce({ items: [AGENT_PROFILE], next: 'second' })
    .mockResolvedValueOnce({
      items: [{ ...AGENT_PROFILE, id: 'agp_second', name: 'Second profile' }],
    })
    .mockResolvedValue({ items: [] });
  render(<ProfilesHome client={{ listProfiles }} onNavigate={vi.fn()} onNewProfile={vi.fn()} />);
  await actor.click(await screen.findByRole('button', { name: 'More profiles' }));
  expect(await screen.findByRole('link', { name: 'Second profile' })).toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'Research agent' })).toBeInTheDocument();
  expect(listProfiles.mock.calls[1]?.[1].cursor).toBe('second');
  await actor.click(screen.getByRole('tab', { name: 'Workspace' }));
  expect(await screen.findByText('No workspace profiles yet.')).toBeInTheDocument();
  expect(screen.queryByRole('link', { name: 'Research agent' })).not.toBeInTheDocument();
});
