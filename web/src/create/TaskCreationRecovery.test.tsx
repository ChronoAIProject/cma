import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { agentClient } from '../test/agent';
import { taskExperience } from '../test/taskExperience';
import { AVAILABLE_CAPACITY } from '../test/capacity';
import { TaskHome } from './TaskHome';
import { lifecycleStorageKey } from './lifecycleIntent';

afterEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
});

it('groups the uncertain receipt and recovery while resending only the original body and key', async () => {
  const create = vi.fn().mockRejectedValue(new TypeError('Response lost'));
  const onCreated = vi.fn();
  render(
    <TaskHome
      viewerId="owner"
      canCreateSandbox
      onCreated={onCreated}
      client={agentClient({
        taskExperience: taskExperience('owner'),
        readTaskCreationSupport: async () => true,
        readTaskTitleModeSupport: async () => true,
        readSandboxCapacity: async () => AVAILABLE_CAPACITY,
        createAgentLifecycle: create,
      })}
    />,
  );
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'Original task' } });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  const recovery = await screen.findByRole('region', { name: 'Task preparation' });
  await within(recovery).findByRole('alert');
  expect(within(recovery).getAllByRole('alert')).toHaveLength(1);
  expect(within(recovery).getByRole('alert')).toHaveTextContent('resend the same request');
  const original = JSON.parse(sessionStorage.getItem(lifecycleStorageKey('owner'))!);
  fireEvent.change(screen.getByLabelText('Follow-up message'), {
    target: { value: 'A separate newer draft' },
  });
  fireEvent.click(within(recovery).getByRole('button', { name: 'Recover original task' }));
  await waitFor(() => expect(create).toHaveBeenCalledTimes(2));
  expect(create.mock.calls[1]![0]).toEqual(create.mock.calls[0]![0]);
  expect(create.mock.calls[1]![1].idempotencyKey).toBe(original.intent.key);
  expect(screen.getByLabelText('Follow-up message')).toHaveValue('A separate newer draft');
  expect(screen.getByRole('region', { name: 'Original request' })).toHaveTextContent(
    'Original task',
  );
  expect(onCreated).not.toHaveBeenCalled();
});

it('keeps unconfirmed setup, last confirmed facts and read-only recovery in one region', async () => {
  sessionStorage.setItem(
    lifecycleStorageKey('owner'),
    JSON.stringify({
      version: 1,
      intent: {
        key: 'synthetic-retained-request',
        body: { name: 'New task', initial_message: 'Retained task' },
        receipt: { id: 'operation', status: 'queued' },
      },
    }),
  );
  const create = vi.fn();
  const read = vi.fn().mockRejectedValue(new TypeError('Status unavailable'));
  const onCreated = vi.fn();
  render(
    <TaskHome
      viewerId="owner"
      canCreateSandbox
      onCreated={onCreated}
      client={agentClient({
        taskExperience: taskExperience('owner'),
        readTaskCreationSupport: async () => true,
        readSandboxCapacity: async () => AVAILABLE_CAPACITY,
        createAgentLifecycle: create,
        readAgentLifecycle: read,
      })}
    />,
  );
  await screen.findByRole('heading', { name: 'Setup updates interrupted' });
  const region = screen.getByRole('region', { name: 'Task preparation' });
  expect(within(region).getByText('Request accepted')).toBeVisible();
  expect(within(region).getAllByRole('alert')).toHaveLength(1);
  const retained = sessionStorage.getItem(lifecycleStorageKey('owner'));
  await act(async () =>
    fireEvent.click(within(region).getByRole('button', { name: 'Retry task status' })),
  );
  expect(read).toHaveBeenCalledTimes(2);
  expect(read.mock.calls.every(([id]) => id === 'operation')).toBe(true);
  expect(create).not.toHaveBeenCalled();
  expect(onCreated).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toBe(retained);
  expect(screen.queryByRole('button', { name: 'Start another task' })).not.toBeInTheDocument();
});
