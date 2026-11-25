import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { agentClient, AGENT_DETAIL } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { taskExperience } from '../test/taskExperience';
import { TaskHome } from './TaskHome';
import { taskDraftStorageKey } from './taskDraft';
import { lifecycleStorageKey } from './lifecycleIntent';
import { lifecycleRecoveryKey } from './lifecycleIntentStorage';
import { AgentLifecycleFollowup } from './AgentLifecycleFollowup';
import type { AgentLifecycleOperationView } from '../api/client';

afterEach(() => {
  cleanup();
  sessionStorage.clear();
  vi.restoreAllMocks();
});

function setup(retained: boolean, status: 'active' | 'provisioning' = 'provisioning') {
  const key = lifecycleStorageKey('owner');
  const saved = JSON.stringify({
    version: 1,
    intent: {
      key: 'retained-old-request',
      body: { name: 'Old Agent' },
      receipt: { id: 'old-operation', status: 'running' },
      agentId: 'agt_1',
    },
  });
  if (retained) sessionStorage.setItem(key, saved);
  let allowRead!: () => void;
  const gate = new Promise<void>((resolve) => {
    allowRead = resolve;
  });
  const create = vi.fn(async () => ({ id: 'new-operation', status: 'queued' }));
  const client = agentClient({
    taskExperience: taskExperience('owner'),
    readTaskCreationSupport: async () => true,
    readTaskRepositorySupport: async () => true,
    readSandboxCapacity: async () => ({
      resource: 'sandbox',
      limit: 10,
      live: 1,
      available: 9,
      counts_by_status: { [status]: 1 },
      observed_at: new Date().toISOString(),
    }),
    createAgentLifecycle: create,
    readAgentLifecycle: async () => ({
      id: 'old-operation',
      status: 'running',
      phase: 'sandbox_runtime',
      agent_id: 'agt_1',
      events: [],
    }),
    readAgent: async () => {
      await gate;
      return {
        ...AGENT_DETAIL,
        status: 'starting',
        sandbox: { ...AGENT_DETAIL.sandbox, status },
        bootstrap: {
          operationId: 'old-operation',
          sandboxId: 'sbx_1',
          workspaceId: 'wsp_1',
        },
      };
    },
    readSandbox: async () => ({ ...sandboxFixture(status), owner: 'owner' }),
  });
  const opened = vi.fn();
  const props = {
    client,
    viewerId: 'owner',
    canCreateSandbox: true,
    onCreated: opened,
  };
  return { props, key, saved, allowRead, opened, create };
}

it.each(['active', 'provisioning'] as const)(
  'keeps New agent independent of a bound old Agent while Sandbox is %s',
  async (status) => {
    const task = setup(true, status);
    const first = render(<TaskHome {...task.props} />);
    fireEvent.change(screen.getByLabelText('Your task'), {
      target: { value: 'hello' },
    });
    await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
    expect(screen.queryByRole('region', { name: 'Submitted task' })).not.toBeInTheDocument();
    expect(screen.queryByText(/Preparing a cloud computer/)).not.toBeInTheDocument();
    await act(async () => task.allowRead());
    expect(task.opened).not.toHaveBeenCalled();
    expect(task.create).not.toHaveBeenCalled();
    first.unmount();
    render(<TaskHome {...task.props} />);
    expect(screen.getByLabelText('Your task')).toHaveValue('hello');
    await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
    expect(task.opened).not.toHaveBeenCalled();
    expect(task.create).not.toHaveBeenCalled();
  },
);

it('keeps fresh home send enabled when the same provisioning Sandbox has no retained creation', async () => {
  const task = setup(false);
  render(<TaskHome {...task.props} />);
  fireEvent.change(screen.getByLabelText('Your task'), {
    target: { value: 'hello' },
  });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
  expect(screen.queryByRole('region', { name: 'Submitted task' })).not.toBeInTheDocument();
  await act(async () => task.allowRead());
  expect(task.opened).not.toHaveBeenCalled();
  expect(task.create).not.toHaveBeenCalled();
});

it('submits hello once with a new key while the bound Agent retains its original identity', async () => {
  const task = setup(true);
  render(<TaskHome {...task.props} />);
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'hello' } });
  const start = screen.getByRole('button', { name: 'Start task' });
  await waitFor(() => expect(start).toBeEnabled());
  fireEvent.click(start);
  fireEvent.click(start);
  await waitFor(() => expect(task.create).toHaveBeenCalledOnce());
  expect(task.create).toHaveBeenCalledWith(
    { name: 'New task', initial_message: 'hello' },
    expect.objectContaining({ idempotencyKey: expect.any(String) }),
  );
  const pending = JSON.parse(sessionStorage.getItem(task.key)!).intent;
  expect(pending.key).not.toBe('retained-old-request');
  expect(pending.body.initial_message).toBe('hello');
  const journal = lifecycleRecoveryKey(
    { owner: 'owner' },
    { agentId: 'agt_1', operationId: 'old-operation' },
  );
  expect(sessionStorage.getItem(journal)).toBe(task.saved);
});

it.each(['succeeded', 'failed', 'waiting_for_connection'] as const)(
  'keeps the new draft and pending request while A reports %s on a mounted follow-up',
  async (status) => {
    const task = setup(true);
    let resolve!: (value: AgentLifecycleOperationView) => void;
    const client = {
      ...task.props.client,
      readAgentLifecycle: vi.fn(
        () =>
          new Promise<AgentLifecycleOperationView>((done) => {
            resolve = done;
          }),
      ),
    };
    const view = render(
      <>
        <AgentLifecycleFollowup client={client} viewerId="owner" agentId="agt_1" />
        <TaskHome {...task.props} />
      </>,
    );
    fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'hello' } });
    await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
    fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
    await waitFor(() => expect(task.create).toHaveBeenCalledOnce());
    const pending = sessionStorage.getItem(task.key);
    const savedDraft = sessionStorage.getItem(taskDraftStorageKey('owner'));
    await act(async () =>
      resolve({
        id: 'old-operation',
        status,
        agent_id: 'agt_1',
        phase: 'agent_startup',
        events: [],
        ...(status === 'waiting_for_connection'
          ? { connection: { services: ['api-github'], target: { type: 'workspace', id: 'wsp_1' } } }
          : {}),
        ...(status === 'failed' ? { error: 'Original setup failed' } : {}),
      }),
    );
    expect(screen.getByRole('region', { name: 'Original request' })).toHaveTextContent('hello');
    expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).toBe(savedDraft);
    expect(sessionStorage.getItem(task.key)).toBe(pending);
    expect(task.opened).not.toHaveBeenCalled();
    expect(task.create).toHaveBeenCalledOnce();
    if (status === 'waiting_for_connection')
      expect(screen.getByRole('button', { name: 'Continue Agent setup' })).toBeVisible();
    if (status === 'failed')
      for (const message of screen.getAllByText('Original setup failed'))
        expect(message).toBeVisible();
    view.unmount();
  },
);

it.each(['capacity', 'permission', 'capability', 'policy', 'identity', 'storage'] as const)(
  'preserves the %s admission gate after isolating a bound request',
  async (gate) => {
    const task = setup(true);
    const client = {
      ...task.props.client,
      ...(gate === 'policy' ? { taskExperience: taskExperience('owner', false) } : {}),
      ...(gate === 'capacity'
        ? {
            readSandboxCapacity: async () => ({
              resource: 'sandbox' as const,
              limit: 10,
              live: 10,
              available: 0,
              counts_by_status: { provisioning: 10 },
              observed_at: new Date().toISOString(),
            }),
          }
        : {}),
      ...(gate === 'capability' ? { readTaskCreationSupport: async () => false } : {}),
    };
    if (gate === 'storage')
      vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
        throw new DOMException('Blocked');
      });
    render(
      <TaskHome
        {...task.props}
        client={client}
        viewerId={gate === 'identity' ? undefined : 'owner'}
        canCreateSandbox={gate !== 'permission'}
      />,
    );
    fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'hello' } });
    await waitFor(() =>
      expect(screen.queryByText('Checking task creation…')).not.toBeInTheDocument(),
    );
    expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
    fireEvent.submit(screen.getByLabelText('Your task').closest('form')!);
    await act(async () => task.allowRead());
    expect(task.create).not.toHaveBeenCalled();
    expect(task.opened).not.toHaveBeenCalled();
    expect(screen.getByLabelText('Your task')).toHaveValue('hello');
  },
);

it('keeps B’s identical submitted draft when the older Agent completes', async () => {
  const task = setup(true);
  const saved = JSON.parse(task.saved);
  saved.intent.body = { name: 'New task', initial_message: 'hello' };
  sessionStorage.setItem(task.key, JSON.stringify(saved));
  const home = <TaskHome {...task.props} />;
  const view = render(home);
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'hello' } });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await waitFor(() => expect(task.create).toHaveBeenCalledOnce());
  const pending = sessionStorage.getItem(task.key);
  const savedDraft = sessionStorage.getItem(taskDraftStorageKey('owner'));
  const client = {
    ...task.props.client,
    readAgentLifecycle: async () => ({
      id: 'old-operation',
      agent_id: 'agt_1',
      status: 'succeeded',
      phase: 'agent_startup',
      events: [],
    }),
  };
  view.rerender(
    <>
      {home}
      <AgentLifecycleFollowup client={client} viewerId="owner" agentId="agt_1" />
    </>,
  );
  const journal = lifecycleRecoveryKey(
    { owner: 'owner' },
    { agentId: 'agt_1', operationId: 'old-operation' },
  );
  await waitFor(() => expect(sessionStorage.getItem(journal)).toBeNull());
  expect(screen.getByRole('region', { name: 'Original request' })).toHaveTextContent('hello');
  expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).toBe(savedDraft);
  expect(sessionStorage.getItem(task.key)).toBe(pending);
  expect(task.opened).not.toHaveBeenCalled();
});
