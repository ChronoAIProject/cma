import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { unavailableClient, type CmaWebClient } from '../api/client';
import { taskExperience } from '../test/taskExperience';
import { AVAILABLE_CAPACITY } from '../test/capacity';
import { TaskHome } from './TaskHome';
import { taskDraftStorageKey, discardSubmittedTaskDraft } from './taskDraft';
import { lifecycleStorageKey } from './lifecycleIntent';
import { compileTaskTemplate, parseTaskTemplate, taskTemplates } from './taskTemplateModel';

afterEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
});
function setup(overrides: Partial<CmaWebClient> = {}, viewerId = 'owner', workspaceId?: string) {
  const create = vi.fn().mockRejectedValue(new Error('Unknown receipt'));
  const client = {
    ...unavailableClient,
    taskExperience: taskExperience(viewerId),
    readTaskCreationSupport: async () => true,
    readTaskTemplateSupport: async () => true,
    readSandboxCapacity: async () => AVAILABLE_CAPACITY,
    createAgentLifecycle: create,
    ...overrides,
  };
  const props = { client, viewerId, workspaceId, canCreateSandbox: true, onCreated: vi.fn() };
  return { ...render(<TaskHome {...props} />), create, props };
}
async function ready() {
  await waitFor(() =>
    expect(screen.queryByText('Checking task creation…')).not.toBeInTheDocument(),
  );
}
function fill(label: string, value: string) {
  fireEvent.change(screen.getByLabelText(label), { target: { value } });
}

it.each(taskTemplates)(
  'validates $id at button, keyboard and direct submit boundaries',
  async (definition) => {
    const { create } = setup();
    await ready();
    fireEvent.click(screen.getByRole('button', { name: definition.name }));
    await waitFor(() => expect(screen.getByLabelText(definition.fields[0]!.label)).toHaveFocus());
    const start = screen.getByRole('button', { name: 'Start task' });
    const form = start.closest('form')!;
    for (const field of definition.fields) {
      for (const value of ['', '   ', field.placeholder]) {
        fill(field.label, value);
        expect(start).toBeDisabled();
        fireEvent.click(start);
        fireEvent.keyDown(screen.getByLabelText(field.label), { key: 'Enter', ctrlKey: true });
        fireEvent.keyDown(screen.getByLabelText(field.label), { key: 'Enter', metaKey: true });
        fireEvent.submit(form);
        expect(create).not.toHaveBeenCalled();
        expect(screen.getByLabelText(field.label)).toHaveFocus();
      }
      fill(field.label, `Synthetic ${field.key}`);
    }
    expect(start).toBeEnabled();
    expect(screen.getByLabelText('Task preview')).toHaveAttribute('readonly');
    fireEvent.keyDown(screen.getByLabelText(definition.fields.at(-1)!.label), {
      key: 'Enter',
      ctrlKey: true,
    });
    await waitFor(() => expect(create).toHaveBeenCalledOnce());
    const template = parseTaskTemplate({
      id: definition.id,
      version: 1,
      fields: Object.fromEntries(definition.fields.map(({ key }) => [key, `Synthetic ${key}`])),
    });
    expect(create.mock.calls[0]![0]).toMatchObject({
      template,
      initial_message: compileTaskTemplate(template).text,
    });
  },
);

it.each([false, undefined, 'failure'] as const)(
  'preserves fields when template support is %s',
  async (support) => {
    const { create } = setup({
      readTaskTemplateSupport:
        support === undefined
          ? undefined
          : async () => {
              if (support === 'failure') throw new Error();
              return support;
            },
    });
    await ready();
    fireEvent.click(screen.getByRole('button', { name: 'Research a topic' }));
    fill('Topic', 'Synthetic topic');
    expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
    fireEvent.submit(screen.getByLabelText('Topic').closest('form')!);
    expect(create).not.toHaveBeenCalled();
    expect(screen.getByLabelText('Topic')).toHaveValue('Synthetic topic');
    fireEvent.click(screen.getByRole('button', { name: 'Write a task without a template' }));
    expect(screen.getByLabelText('Your task')).toHaveValue(
      'Research Synthetic topic and summarize the key findings, with links to sources.',
    );
    expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled();
  },
);

it('restores fields, isolates owner/context, and returns to free text without guessing brackets', async () => {
  const view = setup();
  await ready();
  fill('Your task', 'Discuss [topic], [1, 2] and [label](https://example.com)');
  fireEvent.click(screen.getByRole('button', { name: 'Write a document' }));
  fill('Subject', 'Retained subject');
  view.unmount();
  const restored = setup();
  await ready();
  expect(screen.getByLabelText('Subject')).toHaveValue('Retained subject');
  restored.rerender(<TaskHome {...restored.props} viewerId="other" />);
  expect(screen.getByLabelText('Your task')).toHaveValue('');
  restored.rerender(<TaskHome {...restored.props} workspaceId="workspace-other" />);
  expect(screen.getByLabelText('Your task')).toHaveValue('');
  restored.rerender(<TaskHome {...restored.props} />);
  expect(screen.getByLabelText('Subject')).toHaveValue('Retained subject');
  fireEvent.click(screen.getByRole('button', { name: 'Write a task without a template' }));
  fireEvent.click(screen.getByRole('button', { name: 'Switch and keep draft' }));
  expect(screen.getByLabelText('Your task')).toHaveValue(
    'Discuss [topic], [1, 2] and [label](https://example.com)',
  );
});

it('requires fresh template support when the client changes', async () => {
  const view = setup();
  await ready();
  fireEvent.click(screen.getByRole('button', { name: 'Research a topic' }));
  fill('Topic', 'Synthetic topic');
  expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled();
  const client = {
    ...view.props.client,
    readTaskTemplateSupport: () => new Promise<boolean>(() => {}),
  };
  view.rerender(<TaskHome {...view.props} client={client} />);
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  fireEvent.submit(screen.getByLabelText('Topic').closest('form')!);
  expect(view.create).not.toHaveBeenCalled();
  expect(screen.getByLabelText('Topic')).toHaveValue('Synthetic topic');
});

it.each(['ctrlKey', 'metaKey'])(
  'keeps %s+Enter in the repository dialog from starting a task',
  async (modifier) => {
    const { create } = setup({ readTaskRepositorySupport: async () => true });
    await ready();
    fireEvent.click(screen.getByRole('button', { name: 'Research a topic' }));
    fill('Topic', 'Synthetic topic');
    fireEvent.click(screen.getByRole('button', { name: 'Add git repo' }));
    fill('GitHub repository', 'owner/repository');
    fireEvent.keyDown(screen.getByLabelText('GitHub repository'), {
      key: 'Enter',
      [modifier]: true,
    });
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(create).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
    await waitFor(() => expect(create).toHaveBeenCalledOnce());
    expect(create.mock.calls[0]![0]).toMatchObject({
      repository: { url: 'https://github.com/owner/repository' },
      template: { id: 'research', fields: { topic: 'Synthetic topic' } },
    });
  },
);

it('restores exact untouched version-1 template drafts as unfilled fields', async () => {
  sessionStorage.setItem(
    taskDraftStorageKey('owner'),
    JSON.stringify({ version: 1, text: taskTemplates[0].legacy }),
  );
  const { create } = setup();
  await ready();
  expect(screen.getByLabelText('Topic')).toHaveValue('');
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  fireEvent.submit(screen.getByLabelText('Topic').closest('form')!);
  expect(create).not.toHaveBeenCalled();
});

it('replays the original body and key after reload without recompiling or clearing a newer draft', async () => {
  const view = setup();
  await ready();
  fireEvent.click(screen.getByRole('button', { name: 'Research a topic' }));
  fill('Topic', '  Original topic  ');
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await screen.findByRole('button', { name: 'Recover original task' });
  const original = view.create.mock.calls[0]!;
  fill('Topic', 'A later draft');
  const saved = JSON.parse(sessionStorage.getItem(lifecycleStorageKey('owner'))!) as {
    intent: { body: { initial_message: string }; draftRevision: string };
  };
  saved.intent.body.initial_message = 'Original compiled bytes retained from an older build';
  sessionStorage.setItem(lifecycleStorageKey('owner'), JSON.stringify(saved));
  view.unmount();
  const restored = setup();
  await ready();
  fireEvent.click(screen.getByRole('button', { name: 'Recover original task' }));
  await waitFor(() => expect(restored.create).toHaveBeenCalledOnce());
  expect(restored.create.mock.calls[0]![0]).toEqual({
    ...original[0],
    initial_message: saved.intent.body.initial_message,
  });
  expect(restored.create.mock.calls[0]![1].idempotencyKey).toBe(original[1].idempotencyKey);
  discardSubmittedTaskDraft('owner', original[0], saved.intent.draftRevision);
  expect(screen.getByLabelText('Topic')).toHaveValue('A later draft');
});

it.each([false, true])(
  'clears only matching confirmed template drafts (edited: %s)',
  async (edited) => {
    const view = setup();
    await ready();
    fireEvent.click(screen.getByRole('button', { name: 'Research a topic' }));
    fill('Topic', 'Synthetic topic');
    fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
    await screen.findByRole('button', { name: 'Recover original task' });
    const submitted = view.create.mock.calls[0]![0];
    if (edited) fill('Topic', ' Synthetic topic ');
    const revision = JSON.parse(sessionStorage.getItem(lifecycleStorageKey('owner'))!).intent
      .draftRevision;
    act(() => discardSubmittedTaskDraft('owner', submitted, revision));
    if (edited) {
      expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).not.toBeNull();
      expect(screen.getByLabelText('Topic')).toHaveValue(' Synthetic topic ');
    } else {
      expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).toBeNull();
      await waitFor(() => expect(screen.getByLabelText('Your task')).toHaveValue(''));
    }
  },
);
