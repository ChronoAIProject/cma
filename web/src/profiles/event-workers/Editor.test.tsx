import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import type { EventWorkerProfileRecord } from '@chronoai/cma-sdk';
import corpus from '../../../../api/crates/layer1-public/tests/fixtures/public-contract/event-worker-profiles.json';
import {
  creationClient,
  CREATION_AGENT,
  CREATION_SANDBOX,
  CREATION_WORKSPACE,
} from '../../test/creation';
import { EventWorkerEditor } from './Editor';

const record = corpus.cases[0]!.response.body as unknown as EventWorkerProfileRecord;

it('retains the original creation across navigation and reload, without replaying it until requested', async () => {
  const user = userEvent.setup();
  const create = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('lost response'))
    .mockResolvedValue(record);
  const client = { create, get: vi.fn(), update: vi.fn(), retire: vi.fn() };
  const onNavigate = vi.fn();
  const profiles = creationClient();
  const element = (
    <EventWorkerEditor
      client={client}
      profiles={profiles}
      profileId="new"
      viewerId="owner"
      onNavigate={onNavigate}
    />
  );
  const view = render(element);
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Research worker' } });
  for (const profile of [CREATION_SANDBOX, CREATION_WORKSPACE, CREATION_AGENT]) {
    await user.click(
      await screen.findByRole('button', { name: `Select ${profile.name} Version 1` }),
    );
  }
  await user.click(screen.getByRole('button', { name: 'Create worker profile' }));
  await screen.findByRole('button', { name: 'Retry the same save' });
  expect(screen.getByLabelText('Name')).toBeDisabled();
  view.unmount();
  render(element);
  expect(create).toHaveBeenCalledTimes(1);
  expect(screen.getByLabelText('Name')).toHaveValue('Research worker');
  await user.click(screen.getByRole('button', { name: 'Retry the same save' }));
  await waitFor(() =>
    expect(onNavigate).toHaveBeenCalledWith({ kind: 'event_worker_profile', profileId: record.id }),
  );
  expect(create.mock.calls[1]).toEqual(create.mock.calls[0]);
  expect(sessionStorage.length).toBe(0);
});

it('shows and retires a saved worker even when its source catalogue entries were removed', async () => {
  const user = userEvent.setup();
  const client = {
    create: vi.fn(),
    get: vi.fn().mockResolvedValue(record),
    update: vi.fn(),
    retire: vi.fn().mockResolvedValue(record),
  };
  const onNavigate = vi.fn();
  render(
    <EventWorkerEditor
      client={client}
      profiles={creationClient({
        readProfile: async () => {
          throw new Error('removed');
        },
      })}
      profileId={record.id}
      viewerId="owner"
      onNavigate={onNavigate}
    />,
  );
  await screen.findByText(/Some source profiles are unavailable/);
  expect(screen.getByLabelText('Name')).toHaveValue(record.source.name);
  expect(screen.getByRole('button', { name: 'Save new revision' })).toBeDisabled();
  await user.click(screen.getByRole('button', { name: 'Retire profile' }));
  await user.click(screen.getByRole('button', { name: 'Confirm retirement' }));
  expect(client.retire).toHaveBeenCalledWith(record.id, record.revision);
  expect(onNavigate).toHaveBeenCalledWith({ kind: 'event_worker_profiles' });
});
