import { selectTaskTemplate } from '../test/taskTemplateSelection';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { TaskHome } from './TaskHome';
import { agentClient } from '../test/agent';
import { taskExperience } from '../test/taskExperience';
import { AVAILABLE_CAPACITY } from '../test/capacity';
import { lifecycleStorageKey } from './lifecycleIntent';
import { taskDraftStorageKey } from './taskDraft';

afterEach(() => sessionStorage.clear());

it('preserves compiled template input alongside supported automatic title intent', async () => {
  const create = vi.fn().mockRejectedValue(new TypeError('Lost response'));
  const client = agentClient({
    taskExperience: taskExperience('owner'),
    readTaskCreationSupport: async () => true,
    readTaskTemplateSupport: async () => true,
    readTaskTemplateV2Support: async () => true,
    readTaskTitleModeSupport: async () => true,
    createAgentLifecycle: create,
  });
  render(<TaskHome client={client} viewerId="owner" canCreateSandbox onCreated={vi.fn()} />);
  selectTaskTemplate('Research a topic');
  fireEvent.change(screen.getByLabelText('Topic'), { target: { value: 'Synthetic topic' } });
  const start = screen.getByRole('button', { name: 'Start task' });
  await waitFor(() => expect(start).toBeEnabled());
  fireEvent.click(start);
  await waitFor(() => expect(create).toHaveBeenCalledOnce());
  expect(create.mock.calls[0]?.[0]).toEqual({
    name: 'New task',
    title_mode: 'automatic',
    initial_message:
      'Research the following topic:\n\nSynthetic topic\n\nSummarize the key findings, with links to sources.',
    template: { id: 'research', version: 2, fields: { topic: 'Synthetic topic' } },
  });
  const saved = JSON.parse(sessionStorage.getItem(lifecycleStorageKey('owner'))!);
  const draft = JSON.parse(sessionStorage.getItem(taskDraftStorageKey('owner'))!);
  expect(saved.intent.body).toEqual(create.mock.calls[0]?.[0]);
  expect(saved.intent.draftRevision).toEqual(expect.any(String));
  expect(saved.intent.draftRevision).toBe(draft.revision);
});

it.each([{}, { workspaceId: 'workspace' }, { sandboxId: 'sandbox' }])(
  'requests automatic titles only for new supported task submissions in %j',
  async (context) => {
    const create = vi.fn().mockRejectedValue(new TypeError('Lost response'));
    const client = agentClient({
      taskExperience: taskExperience('owner'),
      readTaskCreationSupport: async () => true,
      readTaskTitleModeSupport: async () => true,
      createAgentLifecycle: create,
    });
    render(
      <TaskHome
        client={client}
        viewerId="owner"
        canCreateSandbox
        onCreated={vi.fn()}
        {...context}
      />,
    );
    fireEvent.change(screen.getByLabelText('Your task'), {
      target: { value: '  Compare tools\n' },
    });
    const start = screen.getByRole('button', { name: 'Start task' });
    await waitFor(() => expect(start).toBeEnabled());
    fireEvent.click(start);
    await waitFor(() => expect(create).toHaveBeenCalledOnce());
    expect(create.mock.calls[0]?.[0]).toEqual({
      name: 'New task',
      title_mode: 'automatic',
      initial_message: '  Compare tools\n',
      ...('workspaceId' in context ? { workspace_id: context.workspaceId } : {}),
      ...('sandboxId' in context ? { sandbox_id: context.sandboxId } : {}),
    });
  },
);

it.each(['absent', 'unsupported', 'failed'])(
  'keeps the legacy request when title mode support is %s',
  async (support) => {
    const create = vi.fn().mockRejectedValue(new TypeError('Lost response'));
    const client = agentClient({
      taskExperience: taskExperience('owner'),
      readTaskCreationSupport: async () => true,
      readTaskTitleModeSupport:
        support === 'absent'
          ? undefined
          : async () => {
              if (support === 'failed') throw new TypeError('Capability unavailable');
              return false;
            },
      createAgentLifecycle: create,
    });
    render(<TaskHome client={client} viewerId="owner" canCreateSandbox onCreated={vi.fn()} />);
    fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'Compare tools' } });
    const start = screen.getByRole('button', { name: 'Start task' });
    await waitFor(() => expect(start).toBeEnabled());
    fireEvent.click(start);
    await waitFor(() => expect(create).toHaveBeenCalledOnce());
    expect(create.mock.calls[0]?.[0]).toEqual({
      name: 'New task',
      initial_message: 'Compare tools',
    });
  },
);

it('waits for capability discovery before capturing a new request', async () => {
  let finish!: (supported: boolean) => void;
  const create = vi.fn().mockRejectedValue(new TypeError('Lost response'));
  const client = agentClient({
    taskExperience: taskExperience('owner'),
    readTaskCreationSupport: async () => true,
    readTaskTitleModeSupport: () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
    createAgentLifecycle: create,
  });
  render(<TaskHome client={client} viewerId="owner" canCreateSandbox onCreated={vi.fn()} />);
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'Compare tools' } });
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  fireEvent.submit(screen.getByLabelText('Your task').closest('form')!);
  expect(create).not.toHaveBeenCalled();
  await act(async () => finish(true));
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await waitFor(() => expect(create).toHaveBeenCalledOnce());
  expect(create.mock.calls[0]?.[0].title_mode).toBe('automatic');
});

it.each([undefined, 'named', 'automatic'] as const)(
  'replays the retained %s title intent unchanged despite changed support and full capacity',
  async (title_mode) => {
    const body = {
      name: 'New task',
      initial_message: 'Original task',
      ...(title_mode ? { title_mode } : {}),
    };
    sessionStorage.setItem(
      lifecycleStorageKey('owner'),
      JSON.stringify({ version: 1, intent: { key: 'original-key', body } }),
    );
    const create = vi.fn().mockRejectedValue(new TypeError('Lost response'));
    const readCapacity = vi.fn(async () => ({
      ...AVAILABLE_CAPACITY,
      live: 10,
      available: 0,
      counts_by_status: { active: 10 },
    }));
    render(
      <TaskHome
        client={agentClient({
          taskExperience: taskExperience('owner'),
          readTaskCreationSupport: async () => true,
          readTaskTitleModeSupport: async () => title_mode !== 'automatic',
          readSandboxCapacity: readCapacity,
          createAgentLifecycle: create,
        })}
        viewerId="owner"
        canCreateSandbox
        onCreated={vi.fn()}
      />,
    );
    await waitFor(() => expect(readCapacity).toHaveBeenCalled());
    expect(screen.queryByRole('button', { name: 'Start task' })).not.toBeInTheDocument();
    fireEvent.click(await screen.findByRole('button', { name: 'Recover original task' }));
    await waitFor(() => expect(create).toHaveBeenCalledOnce());
    expect(create.mock.calls[0]?.[0]).toEqual(body);
    expect(create.mock.calls[0]?.[1].idempotencyKey).toBe('original-key');
  },
);
