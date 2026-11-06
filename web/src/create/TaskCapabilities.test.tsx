import { act, fireEvent, render, renderHook, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { unavailableClient, type CmaWebClient } from '../api/client';
import { useTaskExperience } from '../taskExperience';
import { TaskHome } from './TaskHome';

type PolicyRead = NonNullable<CmaWebClient['taskExperience']>['read'];
type Policy = Awaited<ReturnType<PolicyRead>>;
const unavailable =
  'Task creation is unavailable. You can create an Agent and start a conversation.';

afterEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
});

function pendingPolicy() {
  const requests: { resolve: (value: Policy) => void; reject: (error: Error) => void }[] = [];
  const read = vi.fn<PolicyRead>(
    () =>
      new Promise((resolve, reject) => {
        requests.push({ resolve, reject });
      }),
  );
  const client: CmaWebClient = {
    ...unavailableClient,
    taskExperience: { read },
    readTaskCreationSupport: async () => true,
    readTaskRepositorySupport: async () => true,
    createAgentLifecycle: vi.fn(),
  };
  const settle = async (index: number, admitted: boolean, account = 'owner') => {
    await act(async () =>
      requests[index]!.resolve({
        object: 'cma.ui.task_experience',
        schema_version: 1,
        account_id: account,
        capabilities: {
          task_home_admission: admitted,
          suggested_followup_admission: admitted,
          desktop_enable: admitted,
          browser_takeover: admitted,
          replay_presentation: admitted,
        },
      }),
    );
  };
  return { client, read, requests, settle };
}

function noFallback() {
  expect(screen.queryByText(unavailable)).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'New agent' })).not.toBeInTheDocument();
}

function draw(client: CmaWebClient) {
  const view = render(
    <TaskHome client={client} viewerId="owner" canCreateSandbox onCreated={() => {}} />,
  );
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'Keep this task' } });
  return view;
}

it('never renders the unavailable fallback while the initial policy read is pending or admitted', async () => {
  const policy = pendingPolicy();
  draw(policy.client);
  noFallback();
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  fireEvent.submit(screen.getByLabelText('Your task').closest('form')!);
  expect(policy.client.createAgentLifecycle).not.toHaveBeenCalled();
  await policy.settle(0, true);
  noFallback();
  expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled();
});

it.each(['focus', 'visibilitychange', 'online'])(
  'keeps admitted content stable during a pending %s refresh, then shows a settled refusal',
  async (event) => {
    const policy = pendingPolicy();
    draw(policy.client);
    await policy.settle(0, true);
    fireEvent(event === 'visibilitychange' ? document : window, new Event(event));
    expect(policy.read).toHaveBeenCalledTimes(2);
    noFallback();
    expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled();
    await policy.settle(1, false);
    expect(screen.getByText(unavailable)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'New agent' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  },
);

it.each(['denied', 'failed', 'wrong account', 'absent reader'])(
  'shows the fallback for a settled %s read',
  async (outcome) => {
    const policy = pendingPolicy();
    draw(
      outcome === 'absent reader' ? { ...policy.client, taskExperience: undefined } : policy.client,
    );
    if (outcome === 'failed')
      await act(async () => policy.requests[0]!.reject(new Error('Unavailable')));
    else if (outcome !== 'absent reader')
      await policy.settle(
        0,
        outcome === 'wrong account',
        outcome === 'wrong account' ? 'other' : 'owner',
      );
    expect(screen.getByText(unavailable)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'New agent' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  },
);

it('fails closed when a refresh fails after an admitted result', async () => {
  const policy = pendingPolicy();
  draw(policy.client);
  await policy.settle(0, true);
  fireEvent.focus(window);
  noFallback();
  await act(async () => policy.requests[1]!.reject(new Error('Read failed')));
  expect(screen.getByText(unavailable)).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
});

it('invalidates offline capabilities without misleading fallback copy and waits for an online read', async () => {
  const policy = pendingPolicy();
  draw(policy.client);
  await policy.settle(0, true);
  fireEvent.focus(window);
  vi.spyOn(navigator, 'onLine', 'get').mockReturnValue(false);
  fireEvent(window, new Event('offline'));
  noFallback();
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  await policy.settle(1, true);
  fireEvent.focus(window);
  expect(policy.read).toHaveBeenCalledTimes(2);
  noFallback();
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  vi.spyOn(navigator, 'onLine', 'get').mockReturnValue(true);
  fireEvent(window, new Event('online'));
  noFallback();
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  await policy.settle(2, true);
  expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled();
});

it.each(['client', 'account'])(
  'never exposes previous capabilities on a %s change, including late callbacks',
  async (change) => {
    const first = pendingPolicy();
    const second = pendingPolicy();
    const seen: ReturnType<typeof useTaskExperience>[] = [];
    const view = renderHook(
      ({ client, account }) => {
        const policy = useTaskExperience(client, account);
        seen.push(policy);
        return policy;
      },
      { initialProps: { client: first.client, account: 'owner' } },
    );
    expect(view.result.current.task_home_admission).toBe(false);
    await first.settle(0, true);
    fireEvent.focus(window);
    expect(view.result.current.suggested_followup_admission).toBe(true);
    expect(view.result.current.desktop_enable).toBe(true);
    const boundary = seen.length;
    view.rerender({
      client: change === 'client' ? second.client : first.client,
      account: change === 'account' ? 'other' : 'owner',
    });
    await first.settle(1, true);
    for (const policy of seen.slice(boundary)) {
      expect(policy.task_home_admission).toBe(false);
      expect(policy.suggested_followup_admission).toBe(false);
      expect(policy.desktop_enable).toBe(false);
      expect(policy.browser_takeover).toBe(false);
      expect(policy.replay_presentation).toBe(false);
    }
    if (change === 'client') await second.settle(0, true);
    else await first.settle(2, true, 'other');
    expect(view.result.current.task_home_admission).toBe(true);
  },
);

it('keeps creation and repository checking out of the visible layout while preserving accessible status', async () => {
  const policy = pendingPolicy();
  let creation!: (supported: boolean) => void;
  let repository!: (supported: boolean) => void;
  draw({
    ...policy.client,
    readTaskCreationSupport: () =>
      new Promise((resolve) => {
        creation = resolve;
      }),
    readTaskRepositorySupport: () =>
      new Promise((resolve) => {
        repository = resolve;
      }),
  });
  expect(screen.getByText('Checking task creation…')).toHaveClass('sr-only');
  expect(screen.getByText('Checking repository support…')).toHaveClass('sr-only');
  expect(screen.getByRole('button', { name: 'Add git repo' })).toHaveAccessibleDescription(
    'Checking repository support…',
  );
  await policy.settle(0, true);
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  await act(async () => {
    creation(true);
    repository(true);
  });
  expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled();
  expect(screen.queryByText('Checking task creation…')).not.toBeInTheDocument();
  expect(screen.queryByText('Checking repository support…')).not.toBeInTheDocument();
});
