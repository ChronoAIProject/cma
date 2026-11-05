import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { unavailableClient } from '../api/client';
import { TaskHome } from './TaskHome';

afterEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
});

it('offers ordinary manual Agent creation when task policy is disabled', async () => {
  const create = vi.fn().mockRejectedValue(new TypeError('uncertain receipt'));
  const sdk = createClient({
    fetch: async () =>
      Response.json({
        object: 'cma.ui.task_experience',
        schema_version: 1,
        account_id: 'owner',
        capabilities: {
          task_home_admission: false,
          suggested_followup_admission: false,
          desktop_enable: false,
          browser_takeover: false,
          replay_presentation: false,
        },
      }),
  });
  const client = {
    ...unavailableClient,
    taskExperience: sdk.taskExperience,
    readTaskCreationSupport: async () => true,
    createAgentLifecycle: create,
  };
  render(<TaskHome client={client} viewerId="owner" canCreateSandbox onCreated={() => {}} />);
  await act(async () => {});
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'Policy-gated task' } });
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'New agent' }));
  expect(create).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Create agent' }));
  await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
  expect(create.mock.calls[0]?.[0]).not.toHaveProperty('initial_message');
  expect(create.mock.calls[0]?.[0]).not.toHaveProperty('desktop');
});
