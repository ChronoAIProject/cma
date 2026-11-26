import { selectTaskTemplate } from '../test/taskTemplateSelection';
import type { SandboxCapacity } from '@cma/client';
import type { CmaWebClient } from '../api/client';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AgentLifecycleCreateError } from '../api/client';
import { agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { taskExperience } from '../test/taskExperience';
import { lifecycleStorageKey } from './lifecycleIntent';
import { TaskHome } from './TaskHome';

const full = {
  resource: 'sandbox' as const,
  limit: 10,
  live: 10,
  available: 0,
  counts_by_status: { active: 8, paused: 1, retiring: 1 },
  observed_at: '2026-11-19T12:00:00Z',
};
const free = { ...full, live: 9, available: 1, counts_by_status: { active: 8, paused: 1 } };
function client(overrides: Parameters<typeof agentClient>[0] = {}) {
  return agentClient({
    taskExperience: taskExperience('owner'),
    readTaskCreationSupport: async () => true,
    readSandboxCapacity: async () => full,
    ...overrides,
  });
}
afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
  sessionStorage.clear();
});

it('keeps ten parents counted with eight Agents and browses every owner inventory page', async () => {
  const create = vi.fn();
  const list = vi.fn<CmaWebClient['listSandboxes']>(async (options) =>
    options?.cursor
      ? {
          items: [
            {
              ...sandboxFixture(),
              id: 'empty-paused',
              name: 'Empty paused',
              status: 'paused' as const,
            },
            {
              ...sandboxFixture(),
              id: 'deleting',
              name: 'Deleting parent',
              status: 'retiring' as const,
            },
          ],
        }
      : { items: [{ ...sandboxFixture(), name: 'Eight Agents' }], next: 'page-two' },
  );
  render(
    <TaskHome
      client={client({ createAgentLifecycle: create, listSandboxes: list })}
      viewerId="owner"
      canCreateSandbox
      onCreated={() => {}}
    />,
  );
  expect(await screen.findByText('Environment limit reached')).toBeVisible();
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'Keep this task' } });
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  expect(screen.getByLabelText('Your task')).toBeEnabled();
  fireEvent.click(screen.getByRole('button', { name: 'Environments' }));
  const dialog = await screen.findByRole('dialog', { name: 'Manage environments' });
  expect(await within(dialog).findByRole('link', { name: 'Empty paused' })).toHaveAttribute(
    'href',
    '/sandboxes/empty-paused',
  );
  expect(within(dialog).getByText('Deleting · Sandbox slot in use')).toBeVisible();
  expect(list).toHaveBeenCalledTimes(2);
  expect(list.mock.calls.every(([options]) => options?.excludeRetired === true)).toBe(true);
  expect(create).not.toHaveBeenCalled();
});

it('retains a draft on a confirmed quota refusal and requires a deliberate attempt after capacity recovers', async () => {
  const quota = { code: 'sandbox_limit_reached' as const, limit: 10, live: 10 };
  let capacity: SandboxCapacity = free;
  const create = vi.fn(async () => {
    capacity = full;
    throw new AgentLifecycleCreateError('Quota', 429, quota.code, undefined, quota);
  });
  render(
    <TaskHome
      client={client({ readSandboxCapacity: async () => capacity, createAgentLifecycle: create })}
      viewerId="owner"
      canCreateSandbox
      onCreated={() => {}}
    />,
  );
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'Saved draft' } });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  expect(await screen.findByText(/The Sandbox environment limit is now reached/)).toBeVisible();
  expect(screen.getByLabelText('Your task')).toHaveValue('Saved draft');
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toBeNull();
  expect(screen.queryByRole('button', { name: 'Recover original task' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  capacity = free;
  fireEvent.focus(window);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
  expect(create).toHaveBeenCalledTimes(1);
});

it('keeps unknown capacity explicit while existing-environment creation remains available', async () => {
  const create = vi.fn(async () => ({ id: 'accepted' }));
  const current = client({
    readSandboxCapacity: async () => {
      throw new Error('offline');
    },
    createAgentLifecycle: create,
  });
  const view = render(
    <TaskHome client={current} viewerId="owner" canCreateSandbox onCreated={() => {}} />,
  );
  expect(await screen.findByText(/Sandbox capacity could not be checked/)).toBeVisible();
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'A task' } });
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  view.rerender(
    <TaskHome
      client={current}
      viewerId="owner"
      workspaceId="existing"
      canCreateSandbox
      onCreated={() => {}}
    />,
  );
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'Existing task' } });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
});

it('retains an accepted quota failure until the owner deliberately starts another task', async () => {
  let capacity: SandboxCapacity = free;
  const create = vi.fn(async () => ({ id: 'accepted-before-quota-filled' }));
  render(
    <TaskHome
      client={client({
        readSandboxCapacity: async () => capacity,
        createAgentLifecycle: create,
        readAgentLifecycle: async () => {
          capacity = full;
          return {
            id: 'accepted-before-quota-filled',
            status: 'failed',
            phase: 'failed',
            error: 'Sandbox limit reached',
            failure: { code: 'sandbox_limit_reached', limit: 10, live: 10 },
            events: [],
          };
        },
      })}
      viewerId="owner"
      canCreateSandbox
      onCreated={() => {}}
    />,
  );
  fireEvent.change(screen.getByLabelText('Your task'), {
    target: { value: 'Keep my accepted task' },
  });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  expect(await screen.findByText(/Your task draft is saved/)).toBeVisible();
  expect(screen.getByRole('button', { name: 'Start another task' })).toBeDisabled();
  const retained = sessionStorage.getItem(lifecycleStorageKey('owner'));
  expect(retained).toContain('accepted-before-quota-filled');
  capacity = free;
  fireEvent.focus(window);
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Start another task' })).toBeEnabled(),
  );
  expect(create).toHaveBeenCalledTimes(1);
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toBe(retained);
  fireEvent.click(screen.getByRole('button', { name: 'Start another task' }));
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toBeNull();
  expect(screen.getByLabelText('Your task')).toHaveValue('Keep my accepted task');
  expect(create).toHaveBeenCalledTimes(1);
});

it('revalidates an expired snapshot before fresh submission', async () => {
  const read = vi.fn().mockResolvedValueOnce(free).mockResolvedValue(full);
  const create = vi.fn();
  render(
    <TaskHome
      client={client({ readSandboxCapacity: read, createAgentLifecycle: create })}
      viewerId="owner"
      canCreateSandbox
      onCreated={() => {}}
    />,
  );
  await screen.findByText('1 slot left');
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'Check first' } });
  const now = Date.now();
  vi.spyOn(Date, 'now').mockReturnValue(now + 11_000);
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  expect(await screen.findByText('Environment limit reached')).toBeVisible();
  expect(read).toHaveBeenCalledTimes(2);
  expect(create).not.toHaveBeenCalled();
});

it.each(['ctrlKey', 'metaKey', 'submit'])(
  'revalidates capacity for a template submitted through %s and preserves its fields and automatic title mode',
  async (submission) => {
    let snapshot: SandboxCapacity = free;
    const create = vi.fn(async () => ({ id: 'accepted-template' }));
    render(
      <TaskHome
        client={client({
          readSandboxCapacity: async () => snapshot,
          readTaskTemplateSupport: async () => true,
          readTaskTemplateV2Support: async () => true,
          readTaskTitleModeSupport: async () => true,
          createAgentLifecycle: create,
        })}
        viewerId="owner"
        canCreateSandbox
        onCreated={() => {}}
      />,
    );
    await screen.findByText('1 slot left');
    selectTaskTemplate('Research a topic');
    const topic = screen.getByLabelText('Topic');
    fireEvent.change(topic, { target: { value: 'Retained research topic' } });
    await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
    snapshot = full;
    vi.spyOn(Date, 'now').mockReturnValue(Date.now() + 11_000);
    if (submission === 'submit') fireEvent.submit(topic.closest('form')!);
    else fireEvent.keyDown(topic, { key: 'Enter', [submission]: true });
    expect(await screen.findByText('Environment limit reached')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
    expect(topic).toHaveValue('Retained research topic');
    expect(create).not.toHaveBeenCalled();
    expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toBeNull();
    snapshot = free;
    fireEvent.focus(window);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
    expect(create).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
    await waitFor(() => expect(create).toHaveBeenCalledOnce());
    expect(create).toHaveBeenCalledWith(
      expect.objectContaining({
        title_mode: 'automatic',
        template: { id: 'research', version: 2, fields: { topic: 'Retained research topic' } },
        initial_message:
          'Research the following topic:\n\nRetained research topic\n\nSummarize the key findings, with links to sources.',
      }),
      expect.anything(),
    );
  },
);

it('converges after committed retirement within fifteen seconds without any submission', async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  let snapshot: SandboxCapacity = full;
  const create = vi.fn();
  render(
    <TaskHome
      client={client({ readSandboxCapacity: async () => snapshot, createAgentLifecycle: create })}
      viewerId="owner"
      canCreateSandbox
      onCreated={() => {}}
    />,
  );
  await screen.findByText('Environment limit reached');
  snapshot = free;
  await act(() => vi.advanceTimersByTimeAsync(10_000));
  expect(screen.getByText('1 slot left')).toBeVisible();
  expect(create).not.toHaveBeenCalled();
});
