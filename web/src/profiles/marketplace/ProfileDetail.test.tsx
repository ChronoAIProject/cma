import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { MARKETPLACE_SANDBOX, marketplaceClient } from '../../test/profiles';
import { ProfileDetail } from './ProfileDetail';

it('shows every top-level and referenced revision setting before handing a pinned sandbox to creation', async () => {
  const actor = userEvent.setup();
  const onUse = vi.fn();
  render(
    <ProfileDetail
      client={marketplaceClient()}
      kind="sandbox"
      profileId={MARKETPLACE_SANDBOX.summary.id}
      onNavigate={vi.fn()}
      onUse={onUse}
    />,
  );
  await screen.findByRole('heading', { name: 'Research sandbox' });
  for (const text of [
    '2 cores · 4 GiB',
    'ChronoAI managed',
    'storage',
    'mkdir -p /home/cma/tools',
    'RUN_MODE',
    'mkdir -p reports',
    'Inspect the repository before making changes.',
  ])
    expect(screen.getByText(text)).toBeInTheDocument();
  expect(
    screen.getByRole('heading', { name: 'Research workspace · Revision 1' }),
  ).toBeInTheDocument();
  expect(screen.getByRole('heading', { name: 'Research agent · Revision 1' })).toBeInTheDocument();
  expect(screen.getByText(/Review them before confirming sandbox creation/)).toBeInTheDocument();
  await actor.click(screen.getByRole('button', { name: 'Use' }));
  expect(onUse).toHaveBeenCalledWith({
    kind: 'sandbox',
    profile: { id: MARKETPLACE_SANDBOX.summary.id, revision: 1 },
    entry: MARKETPLACE_SANDBOX,
  });
});

it('forks the exact inspected revision and reconciles an ambiguous result with the same idempotency key', async () => {
  const actor = userEvent.setup();
  const forkProfile = vi
    .fn()
    .mockRejectedValueOnce(new Error('Connection lost'))
    .mockImplementation(marketplaceClient().forkProfile);
  const onNavigate = vi.fn();
  render(
    <ProfileDetail
      client={marketplaceClient({ forkProfile })}
      kind="sandbox"
      profileId={MARKETPLACE_SANDBOX.summary.id}
      onNavigate={onNavigate}
      onUse={vi.fn()}
    />,
  );
  await screen.findByRole('heading', { name: 'Research sandbox' });
  await actor.click(screen.getByRole('button', { name: 'Fork to my profiles' }));
  await screen.findByRole('button', { name: 'Retry fork' });
  expect(screen.getByRole('button', { name: 'Use' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Fork to my profiles' })).toBeDisabled();
  await actor.click(screen.getByRole('button', { name: 'Retry fork' }));
  expect(forkProfile.mock.calls[0]?.slice(0, 3)).toEqual([
    'sandbox',
    MARKETPLACE_SANDBOX.summary.id,
    { revision: 1 },
  ]);
  expect(forkProfile.mock.calls[1]?.[2]).toEqual(forkProfile.mock.calls[0]?.[2]);
  expect(forkProfile.mock.calls[1]?.[3].idempotencyKey).toBe(
    forkProfile.mock.calls[0]?.[3].idempotencyKey,
  );
  expect(onNavigate).toHaveBeenCalledWith({
    kind: 'profile',
    profileKind: 'sandbox',
    profileId: 'sbp_forked',
  });
});

it('does not offer management actions without the owner capability or a creation handoff', async () => {
  render(
    <ProfileDetail
      client={marketplaceClient()}
      kind="sandbox"
      profileId={MARKETPLACE_SANDBOX.summary.id}
      onNavigate={vi.fn()}
      canManage={false}
    />,
  );
  await screen.findByRole('heading', { name: 'Research sandbox' });
  expect(screen.queryByRole('button', { name: 'Fork to my profiles' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Use' })).toBeDisabled();
});
