import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { AgentLifecycleOperationView } from '../api/client';
import { agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { taskExperience } from '../test/taskExperience';
import { AgentLifecycleCard } from './AgentLifecycleCard';
import { AgentLifecycleFollowup } from './AgentLifecycleFollowup';
import { lifecycleStorageKey } from './lifecycleIntent';
import { lifecycleRecoveryKey } from './lifecycleIntentStorage';
import { taskDraftStorageKey, legacyTaskDraftStorageKey } from './taskDraft';
import { TaskHome } from './TaskHome';

const original = {
  version: 1,
  text: '  你可以教我如何使用multi agent进行开发吗\n  ',
  repositoryUrl: 'https://github.com/owner/project',
  repositoryRef: 'feature/task',
  machineIncluded: true,
};
const draftKey = taskDraftStorageKey('owner');
const legacyKey = legacyTaskDraftStorageKey('owner');
function expectCompletedDraft() {
  expect(JSON.parse(sessionStorage.getItem(draftKey)!)).toEqual({
    version: 3,
    text: '',
    repositoryUrl: '',
    repositoryRef: '',
    machineIncluded: false,
    legacyObserved: JSON.stringify(original),
    recoverable: [],
  });
  expect(sessionStorage.getItem(legacyKey)).toBe(JSON.stringify(original));
}
const intentKey = lifecycleStorageKey('owner');
const recoveryKey = lifecycleRecoveryKey(
  { owner: 'owner' },
  { agentId: 'agt_1', operationId: 'draft-operation' },
);

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

async function submit(returnToHome = false) {
  sessionStorage.setItem(legacyKey, JSON.stringify(original));
  let operation: AgentLifecycleOperationView = {
    id: 'draft-operation',
    status: 'running',
    phase: 'agent_startup',
    agent_id: 'agt_1',
    events: [],
  };
  const create = vi.fn(async () => ({ id: operation.id, status: 'queued' }));
  const client = agentClient({
    taskExperience: taskExperience('owner'),
    readTaskCreationSupport: async () => true,
    readTaskRepositorySupport: async () => true,
    createAgentLifecycle: create,
    readAgentLifecycle: async () => operation,
    readSandbox: async () => ({ ...sandboxFixture(), owner: 'owner' }),
  });
  const opened = vi.fn();
  const home = <TaskHome client={client} viewerId="owner" canCreateSandbox onCreated={opened} />;
  const followup = <AgentLifecycleFollowup client={client} viewerId="owner" agentId="agt_1" />;
  const view = render(<>{home}</>);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await waitFor(() => expect(opened).toHaveBeenCalledOnce());
  expect(create).toHaveBeenCalledWith(
    {
      name: 'New task',
      initial_message: original.text,
      repository: { url: original.repositoryUrl, ref: original.repositoryRef },
      machine_included: true,
    },
    expect.anything(),
  );
  expect(sessionStorage.getItem(intentKey)).toBeNull();
  expect(JSON.parse(sessionStorage.getItem(recoveryKey)!).intent.agentId).toBe('agt_1');
  expect(JSON.parse(sessionStorage.getItem(recoveryKey)!).intent.draftRevision).toBe(
    JSON.parse(sessionStorage.getItem(draftKey)!).revision,
  );
  expect(JSON.parse(sessionStorage.getItem(draftKey)!)).toEqual({
    ...original,
    version: 3,
    legacyObserved: JSON.stringify(original),
    recoverable: [],
    revision: expect.any(String),
  });
  // Return to Home after the bound Agent navigation, as the real shell does.
  if (returnToHome) view.unmount();
  const landing = returnToHome ? render(<>{home}</>) : view;
  return {
    client,
    home,
    followup,
    view: landing,
    finish: (status: 'succeeded' | 'failed') => {
      operation = { ...operation, status };
    },
  };
}

it('clears the unchanged submitted draft only after real successful follow-up, then mounts an empty landing', async () => {
  const task = await submit();
  task.view.unmount();
  task.finish('succeeded');
  const conversation = render(task.followup);
  await waitFor(() => expect(sessionStorage.getItem(recoveryKey)).toBeNull());
  expect(sessionStorage.getItem(intentKey)).toBeNull();
  expectCompletedDraft();
  conversation.unmount();
  render(task.home);
  expect(screen.getByLabelText('Your task')).toHaveValue('');
  expect(screen.queryByRole('button', { name: 'Remove repository' })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: /^Cloud computer/ }));
  expect(screen.getByRole('menuitemcheckbox', { name: 'Shared desktop' })).not.toBeChecked();
});

it.each([
  { text: 'My next task' },
  { repositoryUrl: 'https://github.com/owner/another' },
  { repositoryRef: 'another-branch' },
  { machineIncluded: false },
])('preserves the next draft when %j differs from the submitted task', async (change) => {
  const task = await submit();
  task.view.unmount();
  const next = JSON.stringify({
    ...JSON.parse(sessionStorage.getItem(draftKey)!),
    ...change,
    revision: 'next-draft',
  });
  sessionStorage.setItem(draftKey, next);
  task.finish('succeeded');
  const conversation = render(task.followup);
  await waitFor(() => expect(sessionStorage.getItem(recoveryKey)).toBeNull());
  expect(sessionStorage.getItem(intentKey)).toBeNull();
  expect(sessionStorage.getItem(draftKey)).toBe(next);
  conversation.unmount();
  render(task.home);
  expect(screen.getByLabelText('Your task')).toHaveValue({ ...original, ...change }.text);
});

it.each([false, true])(
  'synchronizes a mounted landing without overwriting an edited next draft (%s)',
  async (edited) => {
    const task = await submit(true);
    if (edited)
      fireEvent.change(screen.getByLabelText('Your task'), {
        target: { value: 'Keep my next task' },
      });
    task.finish('succeeded');
    task.view.rerender(
      <>
        {task.home}
        {task.followup}
      </>,
    );
    await waitFor(() => expect(sessionStorage.getItem(recoveryKey)).toBeNull());
    expect(sessionStorage.getItem(intentKey)).toBeNull();
    expect(screen.getByLabelText('Your task')).toHaveValue(edited ? 'Keep my next task' : '');
    if (!edited) {
      expect(screen.queryByRole('button', { name: 'Remove repository' })).not.toBeInTheDocument();
      expectCompletedDraft();
      fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'A fresh task' } });
      expect(JSON.parse(sessionStorage.getItem(draftKey)!)).toEqual({
        version: 3,
        legacyObserved: JSON.stringify(original),
        recoverable: [],
        revision: expect.any(String),
        text: 'A fresh task',
        repositoryUrl: '',
        repositoryRef: '',
        machineIncluded: false,
      });
    } else expect(JSON.parse(sessionStorage.getItem(draftKey)!).text).toBe('Keep my next task');
  },
);

it('keeps a failed preparation draft available through Start another task and reload', async () => {
  const task = await submit();
  task.finish('failed');
  fireEvent.click(await screen.findByRole('button', { name: 'Start another task' }));
  expect(sessionStorage.getItem(intentKey)).toBeNull();
  expect(JSON.parse(sessionStorage.getItem(draftKey)!)).toEqual({
    ...original,
    version: 3,
    legacyObserved: JSON.stringify(original),
    recoverable: [],
    revision: expect.any(String),
  });
  expect(screen.getByLabelText('Your task')).toHaveValue(original.text);
  task.view.unmount();
  render(task.home);
  expect(screen.getByLabelText('Your task')).toHaveValue(original.text);
});

it.each(['getItem', 'setItem'] as const)(
  'does not crash or block the conversation when draft storage %s throws',
  async (method) => {
    const task = await submit();
    task.view.unmount();
    const storage = sessionStorage;
    const fail = vi.fn((key: string) => {
      if (key === draftKey) throw new DOMException('Storage unavailable');
    });
    vi.stubGlobal('sessionStorage', {
      getItem: (key: string) => {
        if (method === 'getItem') fail(key);
        return storage.getItem(key);
      },
      removeItem: storage.removeItem.bind(storage),
      setItem: (key: string, value: string) => {
        if (method === 'setItem') fail(key);
        storage.setItem(key, value);
      },
      clear: storage.clear.bind(storage),
      key: storage.key.bind(storage),
      get length() {
        return storage.length;
      },
    });
    task.finish('succeeded');
    render(
      <>
        <p>Conversation available</p>
        {task.followup}
      </>,
    );
    await waitFor(() => expect(sessionStorage.getItem(recoveryKey)).toBeNull());
    expect(sessionStorage.getItem(intentKey)).toBeNull();
    expect(screen.getByText('Conversation available')).toBeVisible();
    expect(fail).toHaveBeenCalledWith(draftKey);
    expect(JSON.parse(storage.getItem(draftKey)!)).toEqual({
      ...original,
      version: 3,
      legacyObserved: JSON.stringify(original),
      recoverable: [],
      revision: expect.any(String),
    });
  },
);

it('also discards a matching task draft when the manual recovery surface clears confirmed success', async () => {
  const task = await submit();
  task.view.unmount();
  task.finish('succeeded');
  const accepted = JSON.parse(sessionStorage.getItem(recoveryKey)!);
  delete accepted.intent.agentId;
  sessionStorage.removeItem(recoveryKey);
  sessionStorage.setItem(intentKey, JSON.stringify(accepted));
  render(
    <AgentLifecycleCard
      client={task.client}
      viewerId="owner"
      onClose={() => {}}
      onCreated={() => {}}
    />,
  );
  await waitFor(() => expect(sessionStorage.getItem(intentKey)).toBeNull());
  expect(sessionStorage.getItem(intentKey)).toBeNull();
  expectCompletedDraft();
});

it.each([{}, { workspace_id: 'wsp_1' }, { sandbox_id: 'sbx_1' }])(
  'preserves legacy drafts when a successful request has no draft ownership proof: %j',
  async (scope) => {
    const body = { name: 'New task', initial_message: original.text, desktop: true, ...scope };
    const key = legacyTaskDraftStorageKey('owner', scope.workspace_id, scope.sandbox_id);
    const saved = JSON.stringify({ version: 1, text: original.text, desktop: true });
    const otherKeys = [
      legacyTaskDraftStorageKey('another-owner'),
      legacyTaskDraftStorageKey('owner', 'another-workspace'),
    ];
    sessionStorage.setItem(key, saved);
    for (const other of otherKeys) sessionStorage.setItem(other, saved);
    const retained = lifecycleStorageKey('owner', scope.workspace_id, scope.sandbox_id);
    sessionStorage.setItem(
      retained,
      JSON.stringify({
        version: 1,
        intent: {
          key: 'same-request',
          body,
          receipt: { id: 'scope-operation' },
          agentId: 'agt_1',
        },
      }),
    );
    const client = agentClient({
      readAgentLifecycle: async () => ({
        id: 'scope-operation',
        status: 'succeeded',
        phase: 'agent_startup',
        agent_id: 'agt_1',
        events: [],
      }),
    });
    render(<AgentLifecycleFollowup client={client} viewerId="owner" agentId="agt_1" />);
    await waitFor(() => expect(sessionStorage.getItem(retained)).toBeNull());
    await waitFor(() =>
      expect(
        sessionStorage.getItem(
          lifecycleRecoveryKey(
            { owner: 'owner', workspaceId: scope.workspace_id, sandboxId: scope.sandbox_id },
            { agentId: 'agt_1', operationId: 'scope-operation' },
          ),
        ),
      ).toBeNull(),
    );
    expect(sessionStorage.getItem(key)).toBe(saved);
    for (const other of otherKeys) expect(sessionStorage.getItem(other)).toBe(saved);
  },
);

it('preserves an unsubmitted same-body edit in the mounted editor and storage when A completes', async () => {
  const task = await submit(true);
  const originalRevision = JSON.parse(sessionStorage.getItem(draftKey)!).revision;
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'Intervening edit' } });
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: original.text } });
  const edited = sessionStorage.getItem(draftKey);
  expect(JSON.parse(edited!).revision).not.toBe(originalRevision);
  task.finish('succeeded');
  task.view.rerender(
    <>
      {task.home}
      {task.followup}
    </>,
  );
  await waitFor(() => expect(sessionStorage.getItem(recoveryKey)).toBeNull());
  expect(screen.getByLabelText('Your task')).toHaveValue(original.text);
  expect(sessionStorage.getItem(draftKey)).toBe(edited);
  task.view.unmount();
  render(task.home);
  expect(screen.getByLabelText('Your task')).toHaveValue(original.text);
});
