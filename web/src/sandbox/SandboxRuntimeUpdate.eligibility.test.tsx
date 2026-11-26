import { render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { SandboxDetail, SandboxRuntimeUpdate as Update } from '../api/types';
import { agentClient } from '../test/agent';
import { SandboxRuntimeUpdate } from './SandboxRuntimeUpdate';

const latestVersion = 'registry.example/runtime:latest';
const update: Update = {
  currentVersion: 'registry.example/custom:pinned',
  latestVersion,
  state: 'up_to_date',
  updateAvailable: false,
  canUpdate: false,
  preservesHome: false,
  blockedReason:
    'This Sandbox uses a custom image. Automatic CMA runtime updates do not replace custom images.',
  schedule: { timezone: 'Asia/Singapore', localTime: '02:00' },
};

function view(runtimeUpdate: Update, status: SandboxDetail['status'] = 'active', stale = false) {
  return render(
    <SandboxRuntimeUpdate
      client={agentClient()}
      sandboxId="sbx_1"
      ownerId="owner"
      revision={1}
      update={runtimeUpdate}
      sandboxStatus={status}
      stale={stale}
      busy={false}
      onAccepted={vi.fn()}
    />,
  );
}

it.each([update.currentVersion, latestVersion, null])(
  'does not compare an ineligible image %s against managed runtime releases',
  (currentVersion) => {
    view({ ...update, currentVersion, completedAt: '2026-11-01T00:00:00Z' });
    expect(screen.getByText('Managed runtime updates unavailable')).toBeVisible();
    expect(screen.getByText(update.blockedReason!)).toBeVisible();
    expect(screen.getByText('Latest managed runtime')).toBeVisible();
    expect(screen.getByText('Automatic updates').nextElementSibling).toHaveTextContent(
      'Not available for this Sandbox',
    );
    expect(screen.queryByText('Runtime updated')).not.toBeInTheDocument();
    expect(screen.queryByText('Runtime up to date')).not.toBeInTheDocument();
    expect(screen.queryByText('This Sandbox uses the latest runtime.')).not.toBeInTheDocument();
    expect(screen.queryByText(/Daily at/)).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Update runtime' })).not.toBeInTheDocument();
    expect(
      screen.getByRole('region', { name: 'Runtime' }).querySelector('.statedot'),
    ).toHaveAttribute('data-state', 'neutral');
  },
);

it('does not invent a custom image when the retained managed home is unavailable', () => {
  view({
    ...update,
    blockedReason:
      'Runtime updates require a retained managed home and its original runtime identity.',
  });
  expect(screen.getByText('Managed runtime updates unavailable')).toBeVisible();
  expect(screen.getByRole('region', { name: 'Runtime' })).not.toHaveTextContent('custom image');
});

it('keeps current managed runtimes eligible for their ordinary comparison and schedule', () => {
  view({ ...update, currentVersion: latestVersion, preservesHome: true, blockedReason: undefined });
  expect(screen.getByText('Runtime up to date')).toBeVisible();
  expect(screen.getByText('Latest runtime')).toBeVisible();
  expect(screen.getByText('Automatic updates').nextElementSibling).toHaveTextContent(
    'Daily at 02:00 SGT',
  );
});

it.each([
  ['retired', false, 'Environment retired'],
  ['active', true, 'Runtime status unavailable'],
] as const)('preserves %s lifecycle and stale status precedence', (status, stale, title) => {
  view(update, status, stale);
  expect(screen.getByText(title)).toBeVisible();
  expect(screen.queryByText('Managed runtime updates unavailable')).not.toBeInTheDocument();
});

it('uses the observed older version for the managed Desktop update', () => {
  view({
    ...update,
    imageOrigin: 'managed_desktop',
    currentVersion: 'registry.example/runtime:older',
    currentVersionConfirmed: true,
    state: 'available',
    updateAvailable: true,
    canUpdate: true,
    preservesHome: true,
    blockedReason: undefined,
  });
  expect(screen.getByText('Runtime update available')).toBeVisible();
  expect(screen.getByText('registry.example/runtime:older')).toBeVisible();
  expect(screen.getByText('Managed Agent')).toBeVisible();
});

it.each(['managed_desktop', 'managed_machine'] as const)(
  'recognizes a confirmed current version for a paused %s Sandbox',
  (imageOrigin) => {
    view(
      {
        ...update,
        imageOrigin,
        currentVersion: latestVersion,
        currentVersionConfirmed: true,
        preservesHome: true,
        blockedReason: undefined,
      },
      'paused',
    );
    expect(screen.getByRole('status')).toHaveTextContent('Runtime up to date');
  },
);

it('keeps a paused runtime without confirmation unconfirmed', () => {
  view(
    { ...update, currentVersion: latestVersion, preservesHome: true, blockedReason: undefined },
    'paused',
  );
  expect(screen.getByRole('status')).toHaveTextContent('Runtime version unconfirmed');
});

it.each([null, 'registry.example/runtime:last-seen'])(
  'keeps missing or stale Desktop observations (%s) distinct from an available update',
  (currentVersion) => {
    view({
      ...update,
      imageOrigin: 'managed_desktop',
      currentVersion,
      currentVersionConfirmed: false,
      preservesHome: true,
      blockedReason: undefined,
    });
    expect(screen.getByText('Runtime version unconfirmed')).toBeVisible();
    expect(screen.getByText(currentVersion ?? 'Not reported')).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Update runtime' })).not.toBeInTheDocument();
  },
);
