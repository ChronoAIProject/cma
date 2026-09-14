import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { ProfileCard } from './ProfileCard';

const summary = {
  name: 'Research and contribute',
  description: 'Prepare new research artifacts for review.',
  visibility: 'public' as const,
  revision: 3,
  usesCount: 1,
  timestamp: '2026-09-08T12:00:00Z',
  timestampLabel: 'Published' as const,
};

it.each(['sandbox', 'workspace', 'agent'] as const)(
  'renders a named %s card with native navigation and complete metadata',
  async (profileKind) => {
    const onNavigate = vi.fn();
    const route = { kind: 'marketplace_profile', profileKind, profileId: 'profile-1' } as const;
    render(
      <div
        onClick={(event) => {
          if (event.ctrlKey || event.metaKey || event.shiftKey) {
            expect(event.defaultPrevented).toBe(false);
            event.preventDefault();
          }
        }}
      >
        <ProfileCard {...summary} route={route} onNavigate={onNavigate} author="Research team" />
      </div>,
    );
    expect(screen.getByRole('article', { name: summary.name })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: summary.name, level: 2 })).toBeInTheDocument();
    expect(screen.getByText('Public')).toBeInTheDocument();
    expect(screen.getByText('Revision 3')).toBeInTheDocument();
    expect(screen.getByText('1 use')).toBeInTheDocument();
    expect(screen.getByText('By Research team')).toBeInTheDocument();
    expect(screen.getByText(/^Published/)).toHaveAttribute('datetime', summary.timestamp);
    const link = screen.getByRole('link', { name: summary.name });
    expect(link).toHaveAttribute('href', `/marketplace/${profileKind}/profile-1`);
    fireEvent.click(link, { ctrlKey: true });
    fireEvent.click(link, { metaKey: true });
    fireEvent.click(link, { shiftKey: true });
    expect(onNavigate).not.toHaveBeenCalled();
    const actor = userEvent.setup();
    link.focus();
    await actor.keyboard('{Enter}');
    expect(onNavigate).toHaveBeenCalledExactlyOnceWith(route);
  },
);

it('keeps retired unpublished private profiles identifiable without exposing an owner identifier', () => {
  render(
    <ProfileCard
      {...summary}
      name="A long original profile name that remains the accessible link name"
      description=""
      route={{ kind: 'profile', profileKind: 'agent', profileId: 'private-profile' }}
      onNavigate={vi.fn()}
      revision={undefined}
      visibility="private"
      retired
      usesCount={0}
      timestampLabel="Updated"
    />,
  );
  expect(screen.getByText('Unpublished')).toBeInTheDocument();
  expect(screen.getByText('Private')).toBeInTheDocument();
  expect(screen.getByText('Retired')).toBeInTheDocument();
  expect(screen.getByText('0 uses')).toBeInTheDocument();
  expect(screen.queryByText(/^By /)).not.toBeInTheDocument();
  expect(screen.getByRole('link')).toHaveAccessibleName(
    'A long original profile name that remains the accessible link name',
  );
});
