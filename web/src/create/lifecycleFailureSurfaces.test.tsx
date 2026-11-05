import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { CmaWebClient } from '../api/client';
import { connectionClient } from '../connections/client';
import { agentClient } from '../test/agent';
import { taskExperience } from '../test/taskExperience';
import { AgentLifecycleCard } from './AgentLifecycleCard';
import { AgentLifecycleFollowup } from './AgentLifecycleFollowup';
import { TaskHome } from './TaskHome';
import { lifecycleStorageKey } from './lifecycleIntent';

afterEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
});

const key = lifecycleStorageKey('owner');
const request = (task: boolean) => ({
  name: 'Saved creation',
  ...(task ? { initial_message: 'Saved task' } : {}),
});
const retained = (task: boolean, accepted: boolean) => {
  sessionStorage.setItem(
    key,
    JSON.stringify({
      version: 1,
      intent: {
        key: 'original-key',
        body: request(task),
        ...(accepted ? { receipt: { id: 'operation' }, agentId: 'agt_1' } : {}),
      },
    }),
  );
};

function show(surface: 'task' | 'manual' | 'followup', client: CmaWebClient) {
  if (surface === 'task')
    return render(
      <TaskHome client={client} viewerId="owner" canCreateSandbox onCreated={vi.fn()} />,
    );
  if (surface === 'manual')
    return render(
      <AgentLifecycleCard
        client={client}
        viewerId="owner"
        canCreateSandbox
        onCreated={vi.fn()}
        onClose={vi.fn()}
      />,
    );
  return render(<AgentLifecycleFollowup client={client} viewerId="owner" agentId="agt_1" />);
}

const surfaces = [
  { surface: 'task', label: 'Retry task status' },
  { surface: 'manual', label: 'Retry status' },
  { surface: 'followup', label: 'Retry setup status' },
] as const;

it.each(surfaces.flatMap((row) => [true, false].map((task) => ({ ...row, task }))))(
  '$surface names its visible status action for a retained task=$task',
  async ({ surface, label, task }) => {
    retained(task, true);
    const create = vi.fn();
    const read = vi.fn().mockRejectedValueOnce(new TypeError('Failed to fetch')).mockResolvedValue({
      id: 'operation',
      agent_id: 'agt_1',
      status: 'failed',
      phase: 'agent_startup',
      events: [],
    });
    show(
      surface,
      agentClient({
        createAgentLifecycle: create,
        readAgentLifecycle: read,
        readTaskCreationSupport: async () => true,
        taskExperience: taskExperience('owner'),
      }),
    );
    const button = await screen.findByRole('button', { name: label });
    expect(button).toBeVisible();
    expect(screen.getByRole('alert')).toHaveTextContent(
      `${task ? 'Task' : 'Agent'} status couldn't be read. Use ${label} to check again.`,
    );
    fireEvent.click(button);
    await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
    expect(create).not.toHaveBeenCalled();
    expect(sessionStorage.getItem(key)).toContain('original-key');
  },
);

it.each(
  (
    [
      { surface: 'task', label: 'Recover original task' },
      { surface: 'manual', label: 'Recover creation' },
    ] as const
  ).flatMap((row) => [true, false].map((task) => ({ ...row, task }))),
)(
  '$surface names its visible recovery action for a retained task=$task',
  async ({ surface, label, task }) => {
    retained(task, false);
    const create = vi.fn().mockRejectedValue(new TypeError('Failed to fetch'));
    show(
      surface,
      agentClient({ createAgentLifecycle: create, taskExperience: taskExperience('owner') }),
    );
    fireEvent.click(await screen.findByRole('button', { name: label }));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      `We couldn't confirm your ${task ? 'task was received' : 'Agent was created'}. Use ${label} to resend the same request.`,
    );
    expect(screen.getByRole('button', { name: label })).toBeVisible();
    fireEvent.click(screen.getByRole('button', { name: label }));
    await waitFor(() => expect(create).toHaveBeenCalledTimes(2));
    expect(create.mock.calls[1]?.[1].idempotencyKey).toBe('original-key');
    expect(create.mock.calls[1]?.[0]).toEqual(request(task));
  },
);

it.each([true, false])(
  'follow-up offers its visible status action after an uncertain continuation, task=%s',
  async (task) => {
    retained(task, true);
    const target = { type: 'workspace' as const, id: 'wsp_1' };
    const read = vi.fn().mockResolvedValue({
      id: 'operation',
      agent_id: 'agt_1',
      status: 'waiting_for_connection',
      phase: 'agent_startup',
      connection: { services: ['api-github'], target },
      events: [],
    });
    const create = vi.fn().mockRejectedValue(new TypeError('Failed to fetch'));
    vi.spyOn(connectionClient, 'check').mockResolvedValue({
      owner: 'owner',
      ready: true,
      checked_at: '2026-11-05T00:00:00Z',
      services: [],
    });
    show('followup', agentClient({ readAgentLifecycle: read, createAgentLifecycle: create }));
    fireEvent.click(await screen.findByRole('button', { name: 'Continue Agent setup' }));
    expect(
      await screen.findByText(
        `We couldn't confirm your ${task ? 'task' : 'Agent'} setup. Use Retry setup status to check again.`,
      ),
    ).toBeVisible();
    const retry = screen.getByRole('button', { name: 'Retry setup status' });
    expect(retry).toBeVisible();
    fireEvent.click(retry);
    await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
    expect(create).toHaveBeenCalledTimes(1);
    expect(create.mock.calls[0]?.[0]).toEqual(request(task));
    expect(create.mock.calls[0]?.[1].idempotencyKey).toBe('original-key');
    expect(sessionStorage.getItem(key)).toContain('original-key');
  },
);
