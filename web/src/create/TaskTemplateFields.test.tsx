import { act, fireEvent, render, renderHook, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { unavailableClient, type CmaWebClient } from '../api/client';
import { taskExperience } from '../test/taskExperience';
import { selectTaskTemplate } from '../test/taskTemplateSelection';
import { AVAILABLE_CAPACITY } from '../test/capacity';
import { TaskHome } from './TaskHome';
import {
  taskDraftStorageKey,
  legacyTaskDraftStorageKey,
  discardSubmittedTaskDraft,
  useTaskDraft,
} from './taskDraft';
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
    readTaskTemplateV2Support: async () => true,
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
    selectTaskTemplate(definition.name);
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
      fill(
        field.label,
        field.key === 'link' ? 'https://example.com/synthetic.csv' : `Synthetic ${field.key}`,
      );
    }
    expect(start).toBeEnabled();
    expect(screen.getByRole('region', { name: 'Task preview · read-only' })).toBeVisible();
    expect(screen.queryByRole('textbox', { name: /Task preview/ })).toBeNull();
    fireEvent.keyDown(screen.getByLabelText(definition.fields.at(-1)!.label), {
      key: 'Enter',
      ctrlKey: true,
    });
    await waitFor(() => expect(create).toHaveBeenCalledOnce());
    const template = parseTaskTemplate({
      id: definition.id,
      version: 2,
      fields: Object.fromEntries(
        definition.fields.map(({ key }) => [
          key,
          key === 'link' ? 'https://example.com/synthetic.csv' : `Synthetic ${key}`,
        ]),
      ),
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
      readTaskTemplateV2Support: async () => false,
      readTaskTemplateSupport:
        support === undefined
          ? undefined
          : async () => {
              if (support === 'failure') throw new Error();
              return support;
            },
    });
    await ready();
    selectTaskTemplate('Research a topic');
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
  selectTaskTemplate('Write a document');
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
  selectTaskTemplate('Research a topic');
  fill('Topic', 'Synthetic topic');
  expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled();
  const client = {
    ...view.props.client,
    readTaskTemplateSupport: () => new Promise<boolean>(() => {}),
    readTaskTemplateV2Support: () => new Promise<boolean>(() => {}),
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
    selectTaskTemplate('Research a topic');
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
    legacyTaskDraftStorageKey('owner'),
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
  selectTaskTemplate('Research a topic');
  fill('Topic', '  Original topic  ');
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await screen.findByRole('button', { name: 'Recover original task' });
  const original = view.create.mock.calls[0]!;
  expect(screen.queryByLabelText('Topic')).not.toBeInTheDocument();
  const later = renderHook(() => useTaskDraft('owner'));
  act(() =>
    later.result.current.changeTemplate({
      id: 'research',
      version: 1,
      fields: { topic: 'A later draft' },
    }),
  );
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
  expect(later.result.current.template).toMatchObject({ fields: { topic: 'A later draft' } });
  expect(
    JSON.parse(sessionStorage.getItem(taskDraftStorageKey('owner'))!).template.fields.topic,
  ).toBe('A later draft');
  expect(screen.getByRole('region', { name: 'Original request' }).textContent).toBe(
    saved.intent.body.initial_message,
  );
});

it.each([false, true])(
  'clears only matching confirmed template drafts (edited: %s)',
  async (edited) => {
    const view = setup();
    await ready();
    selectTaskTemplate('Research a topic');
    fill('Topic', 'Synthetic topic');
    fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
    await screen.findByRole('button', { name: 'Recover original task' });
    const submitted = view.create.mock.calls[0]![0];
    expect(screen.queryByLabelText('Topic')).not.toBeInTheDocument();
    const retained = renderHook(() => useTaskDraft('owner'));
    if (edited)
      act(() =>
        retained.result.current.changeTemplate({
          id: 'research',
          version: 1,
          fields: { topic: ' Synthetic topic ' },
        }),
      );
    const revision = JSON.parse(sessionStorage.getItem(lifecycleStorageKey('owner'))!).intent
      .draftRevision;
    act(() => discardSubmittedTaskDraft('owner', submitted, revision));
    if (edited) {
      expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).not.toBeNull();
      expect(retained.result.current.template).toMatchObject({
        fields: { topic: ' Synthetic topic ' },
      });
    } else {
      expect(JSON.parse(sessionStorage.getItem(taskDraftStorageKey('owner'))!)).toEqual({
        version: 3,
        text: '',
        repositoryUrl: '',
        repositoryRef: '',
        machineIncluded: false,
        legacyObserved: null,
        recoverable: [],
      });
      expect(retained.result.current.template).toBeUndefined();
      expect(retained.result.current.text).toBe('');
    }
  },
);

it.each([
  {
    name: 'Research a topic',
    fields: [
      [
        'Topic',
        'Compare three established methods. Cite public sources and provide a concise table.',
      ],
    ],
    expected:
      'Research the following topic:\n\nCompare three established methods. Cite public sources and provide a concise table.\n\nSummarize the key findings, with links to sources.',
  },
  {
    name: 'Write code',
    fields: [
      ['Language', 'JavaScript'],
      ['What should the code do?', 'Create a small accessible form on a new branch.'],
    ],
    expected:
      'Write a JavaScript script for the following task:\n\nCreate a small accessible form on a new branch.',
  },
])(
  'previews and submits grammatical $name instructions without rewriting user sentences',
  async ({ name, fields, expected }) => {
    const { create } = setup();
    await ready();
    selectTaskTemplate(name);
    for (const [label, value] of fields) fill(label!, value!);
    expect(screen.getByRole('region', { name: 'Task preview · read-only' })).toHaveTextContent(
      expected.replace(/\s+/g, ' '),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
    await waitFor(() => expect(create).toHaveBeenCalledOnce());
    expect(create.mock.calls[0]![0]).toMatchObject({
      initial_message: expected,
      template: { version: 2 },
    });
  },
);

const researchTopic =
  'Compare three established methods for testing AI agent browser reliability. Cite public sources and provide a concise table.';
const codeBehavior = 'Create a small accessible page on a new branch.';

describe.each([
  {
    id: 'research',
    name: 'Research a topic',
    values: [['Topic', researchTopic]],
    fields: { topic: researchTopic },
    corrected: `Research the following topic:\n\n${researchTopic}\n\nSummarize the key findings, with links to sources.`,
    legacy: `Research ${researchTopic} and summarize the key findings, with links to sources.`,
  },
  {
    id: 'code',
    name: 'Write code',
    values: [
      ['Language', 'JavaScript'],
      ['What should the code do?', codeBehavior],
    ],
    fields: { language: 'JavaScript', behavior: codeBehavior },
    corrected: `Write a JavaScript script for the following task:\n\n${codeBehavior}`,
    legacy: `Write a JavaScript script that ${codeBehavior}.`,
  },
] as const)(
  '$name before template support settles',
  ({ id, name, values, fields, corrected, legacy }) => {
    it.each([true, false])(
      'previews and submits the same text when v2 support is %s',
      async (supportsV2) => {
        let finishDiscovery!: (supported: boolean) => void;
        const { create } = setup({
          readTaskTemplateV2Support: () =>
            new Promise<boolean>((resolve) => {
              finishDiscovery = resolve;
            }),
        });
        await ready();
        selectTaskTemplate(name);
        for (const [label, value] of values) fill(label, value);
        const preview = screen.getByRole('region', { name: 'Task preview · read-only' });
        const start = screen.getByRole('button', { name: 'Start task' });
        expect(preview.textContent).toBe(corrected);
        expect(start).toBeDisabled();
        fireEvent.submit(start.closest('form')!);
        expect(create).not.toHaveBeenCalled();

        await act(async () => finishDiscovery(supportsV2));
        const expected = supportsV2 ? corrected : legacy;
        expect(preview.textContent).toBe(expected);
        expect(start).toBeEnabled();
        expect(
          JSON.parse(sessionStorage.getItem(taskDraftStorageKey('owner'))!).template.version,
        ).toBe(1);
        fireEvent.click(start);
        await waitFor(() => expect(create).toHaveBeenCalledOnce());
        const template = { id, version: supportsV2 ? 2 : 1, fields };
        expect(create.mock.calls[0]![0]).toMatchObject({ initial_message: expected, template });
        expect(JSON.parse(sessionStorage.getItem(taskDraftStorageKey('owner'))!).template).toEqual(
          template,
        );
      },
    );
  },
);
