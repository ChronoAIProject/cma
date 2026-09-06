import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import type { CreationProfile } from './client';
import { creationClient, CREATION_AGENT } from '../test/creation';
import { ProfilePicker } from './ProfilePicker';

it('never reports an empty catalogue before the first request resolves', async () => {
  let complete: ((value: { items: [] }) => void) | undefined;
  const client = creationClient({
    listProfiles: () =>
      new Promise((resolve) => {
        complete = resolve;
      }),
  });
  render(<ProfilePicker client={client} kind="agent" onChange={vi.fn()} />);
  expect(screen.getByRole('status')).toHaveTextContent('Loading profiles');
  expect(screen.queryByText('No published profiles')).not.toBeInTheDocument();
  await act(async () => complete?.({ items: [] }));
  expect(screen.getByText('No published profiles')).toBeInTheDocument();
});

it('ignores an aborted selection after the owner changes profile source', async () => {
  const user = userEvent.setup();
  let complete: ((value: CreationProfile) => void) | undefined;
  let signal: AbortSignal | undefined;
  const onChange = vi.fn();
  const client = creationClient({
    readProfile: (_kind, _choice, options) => {
      signal = options?.signal;
      return new Promise((resolve) => {
        complete = resolve;
      });
    },
  });
  render(<ProfilePicker client={client} kind="agent" onChange={onChange} />);
  await user.click(await screen.findByRole('button', { name: /Research agent Revision/ }));
  await user.click(screen.getByRole('radio', { name: 'Public' }));
  expect(signal?.aborted).toBe(true);
  await act(async () => complete?.(CREATION_AGENT));
  expect(onChange).toHaveBeenCalledWith(undefined);
  expect(onChange).not.toHaveBeenCalledWith(CREATION_AGENT);
});

it('pages profiles without duplicating entries and rejects a stalled cursor', async () => {
  const user = userEvent.setup();
  const choice = { id: 'agent-1', revision: 1, source: 'own' as const, name: 'First agent' };
  const listProfiles = vi
    .fn()
    .mockResolvedValueOnce({ items: [choice], next: 'next' })
    .mockResolvedValueOnce({
      items: [choice, { ...choice, id: 'agent-2', name: 'Second agent' }],
      next: 'next',
    });
  render(
    <ProfilePicker client={creationClient({ listProfiles })} kind="agent" onChange={vi.fn()} />,
  );
  await user.click(await screen.findByRole('button', { name: 'Load more' }));
  await waitFor(() =>
    expect(screen.getByRole('alert')).toHaveTextContent('More profiles could not be loaded.'),
  );
  expect(screen.getAllByRole('button', { name: /First agent Revision/ })).toHaveLength(1);
});
