import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { unavailableClient, type CmaWebClient } from '../api/client';
import { taskExperience } from '../test/taskExperience';
import { AVAILABLE_CAPACITY } from '../test/capacity';
import { TaskHome } from './TaskHome';
import {
  discardSubmittedTaskDraft,
  taskDraftStorageKey,
  legacyTaskDraftStorageKey,
} from './taskDraft';
import { emptyTemplate, taskTemplates } from './taskTemplateModel';
import { retainTaskViewer } from './taskIdentity';
import { selectTaskTemplate } from '../test/taskTemplateSelection';

afterEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});
function setup(overrides: Partial<CmaWebClient> = {}) {
  const create = vi.fn();
  const props = {
    client: {
      ...unavailableClient,
      taskExperience: taskExperience('owner'),
      readTaskCreationSupport: async () => true,
      readTaskTemplateSupport: async () => true,
      readTaskTemplateV2Support: async () => true,
      readSandboxCapacity: async () => AVAILABLE_CAPACITY,
      createAgentLifecycle: create,
      ...overrides,
    },
    viewerId: 'owner',
    canCreateSandbox: true,
    onCreated: vi.fn(),
  };
  return { ...render(<TaskHome {...props} />), props, create };
}
const click = (name: string) =>
  taskTemplates.some((template) => template.name === name)
    ? selectTaskTemplate(name)
    : fireEvent.click(screen.getByRole('button', { name }));
const fill = (label: string, value: string) =>
  fireEvent.change(screen.getByLabelText(label), { target: { value } });
const draftRevision = (): string =>
  JSON.parse(sessionStorage.getItem(taskDraftStorageKey('owner'))!).revision;

it('keeps a partial template through confirmed switching, reload and resume', async () => {
  const view = setup();
  await waitFor(() => expect(screen.queryByText('Checking task creation…')).toBeNull());
  fill('Your task', 'My separate free-text draft');
  click('Write a document');
  fill('Document type', ' report ');
  fill('Subject', '草稿 [subject]');
  click('Write a task without a template');
  const dialog = screen.getByRole('dialog', { name: 'Keep this template draft?' });
  fireEvent.click(within(dialog).getByRole('button', { name: 'Keep editing template' }));
  expect(screen.getByLabelText('Subject')).toHaveValue('草稿 [subject]');
  click('Write a task without a template');
  click('Switch and keep draft');
  expect(screen.getByLabelText('Your task')).toHaveValue('My separate free-text draft');
  view.unmount();
  render(<TaskHome {...view.props} />);
  click('Resume document template');
  expect(screen.getByLabelText('Document type')).toHaveValue(' report ');
  expect(screen.getByLabelText('Subject')).toHaveValue('草稿 [subject]');
  expect(screen.getByLabelText('Audience')).toHaveValue('');
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  expect(view.create).not.toHaveBeenCalled();
});

it('keeps each template independently when switching back and forth', async () => {
  const view = setup();
  click('Write a document');
  fill('Subject', 'Document draft');
  click('Write code');
  fill('Language', 'Python');
  click('Write a document');
  expect(screen.getByLabelText('Subject')).toHaveValue('Document draft');
  click('Write code');
  expect(screen.getByLabelText('Language')).toHaveValue('Python');
  expect(view.create).not.toHaveBeenCalled();
});

it('converts complete fields exactly, retaining their original whitespace after reload', async () => {
  const view = setup();
  click('Write a document');
  fill('Document type', ' report ');
  fill('Subject', '  草稿 [subject]  ');
  fill('Audience', 'readers');
  const preview = 'Write a report about 草稿 [subject] for readers.';
  click('Write a task without a template');
  expect(screen.getByLabelText('Your task')).toHaveValue(preview);
  view.unmount();
  render(<TaskHome {...view.props} />);
  expect(screen.getByLabelText('Your task')).toHaveValue(preview);
  click('Resume document template');
  expect(screen.getByLabelText('Subject')).toHaveValue('  草稿 [subject]  ');
  expect(view.create).not.toHaveBeenCalled();
});

it('requires explicit confirmation before replacing an existing free-text draft', async () => {
  const view = setup();
  fill('Your task', 'Keep this until I choose replacement');
  click('Research a topic');
  fill('Topic', 'A complete template');
  click('Write a task without a template');
  const saved = sessionStorage.getItem(taskDraftStorageKey('owner'));
  const dialog = screen.getByRole('dialog', { name: 'Replace your free-text draft?' });
  fireEvent.keyDown(dialog, { key: 'Escape' });
  expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).toBe(saved);
  expect(screen.getByLabelText('Topic')).toHaveValue('A complete template');
  click('Write a task without a template');
  click('Replace free-text draft');
  expect(screen.getByLabelText('Your task')).toHaveValue(
    'Research A complete template and summarize the key findings, with links to sources.',
  );
  click('Resume research template');
  expect(screen.getByLabelText('Topic')).toHaveValue('A complete template');
  expect(view.create).not.toHaveBeenCalled();
});

it.each(['QuotaExceededError', 'SecurityError'])(
  'retains the current fields and durable draft when storage throws %s',
  async (name) => {
    const view = setup();
    click('Write a document');
    fill('Subject', 'Saved subject');
    const key = taskDraftStorageKey('owner');
    const saved = sessionStorage.getItem(key);
    const original = Storage.prototype.setItem;
    const deny = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (
      this: Storage,
      key,
      value,
    ) {
      if (key.startsWith('cma:task-draft:')) throw new DOMException('Synthetic refusal', name);
      return original.call(this, key, value);
    });
    fill('Subject', 'New fields must stay visible');
    click('Write code');
    expect(screen.getByLabelText('Subject')).toHaveValue('New fields must stay visible');
    click('Write a task without a template');
    click('Switch and keep draft');
    const dialog = screen.getByRole('dialog');
    expect(within(dialog).getByRole('alert')).toHaveTextContent('could not be saved');
    expect(screen.getByLabelText('Subject')).toHaveValue('New fields must stay visible');
    expect(sessionStorage.getItem(key)).toBe(saved);
    const copy = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal('navigator', { ...navigator, clipboard: { writeText: copy } });
    click('Copy filled details');
    await screen.findByText('Filled details copied.');
    expect(copy).toHaveBeenCalledWith('Subject: New fields must stay visible');
    deny.mockRestore();
    click('Switch and keep draft');
    click('Resume document template');
    expect(screen.getByLabelText('Subject')).toHaveValue('New fields must stay visible');
    expect(view.create).not.toHaveBeenCalled();
  },
);

it('provides selectable details if both storage and clipboard are unavailable', async () => {
  setup();
  click('Write a document');
  fill('Document type', 'Report');
  fill('Subject', 'Synthetic details');
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new Error('Full');
  });
  vi.stubGlobal('navigator', { ...navigator, clipboard: undefined });
  click('Write a task without a template');
  click('Switch and keep draft');
  click('Copy filled details');
  expect(
    screen.getByLabelText('Copy is unavailable. Select and copy your filled details.'),
  ).toHaveValue('Document type: Report\nSubject: Synthetic details');
  click('Keep editing template');
  expect(screen.getByLabelText('Subject')).toHaveValue('Synthetic details');
});

it.each([
  '{invalid',
  JSON.stringify({ version: 99, text: 'Newer draft' }),
  JSON.stringify({
    version: 3,
    legacyObserved: null,
    recoverable: [],
    text: 'Saved text',
    templates: [emptyTemplate('code'), emptyTemplate('code')],
  }),
  JSON.stringify({
    version: 3,
    legacyObserved: null,
    recoverable: [],
    text: 'Saved text',
    templates: [{ id: 'document', fields: {} }],
  }),
])('does not overwrite an unreadable stored draft: %s', (raw) => {
  const key = taskDraftStorageKey('owner');
  sessionStorage.setItem(key, raw);
  const { create } = setup();
  fill('Your task', 'In-memory text');
  click('Write a document');
  expect(screen.getByLabelText('Your task')).toHaveValue('In-memory text');
  expect(sessionStorage.getItem(key)).toBe(raw);
  expect(create).not.toHaveBeenCalled();
});

it('does not overwrite a draft after an initial storage read fails and later recovers', () => {
  const key = taskDraftStorageKey('owner');
  const raw = JSON.stringify({
    version: 3,
    legacyObserved: null,
    recoverable: [],
    text: 'Unseen saved text',
  });
  sessionStorage.setItem(key, raw);
  const original = Storage.prototype.getItem;
  const deny = vi.spyOn(Storage.prototype, 'getItem').mockImplementation(function (
    this: Storage,
    key,
  ) {
    if (key.startsWith('cma:task-draft:')) throw new Error('Synthetic storage failure');
    return original.call(this, key);
  });
  setup();
  deny.mockRestore();
  fill('Your task', 'New text before reload');
  click('Write a document');
  expect(sessionStorage.getItem(key)).toBe(raw);
  expect(screen.getByLabelText('Your task')).toHaveValue('New text before reload');
});

it.each([1, 2])(
  'retains version-%s templates without making legacy placeholders runnable',
  (version) => {
    sessionStorage.setItem(
      legacyTaskDraftStorageKey('owner'),
      JSON.stringify({
        version,
        text: version === 1 ? taskTemplates[0].legacy : 'Separate text',
        ...(version === 2
          ? {
              template: {
                ...emptyTemplate('document'),
                fields: { type: '', subject: 'Old subject', audience: '' },
              },
            }
          : {}),
      }),
    );
    setup();
    click('Write a task without a template');
    if (version === 2) click('Switch and keep draft');
    else expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.getByLabelText('Your task')).toHaveValue(version === 1 ? '' : 'Separate text');
    click(version === 1 ? 'Resume research template' : 'Resume document template');
    expect(screen.getByLabelText(version === 1 ? 'Topic' : 'Subject')).toHaveValue(
      version === 1 ? '' : 'Old subject',
    );
    expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  },
);

it('keeps drafts isolated by owner and task scope and clears them on account change', () => {
  retainTaskViewer('owner');
  const view = setup();
  click('Write a document');
  fill('Subject', 'Home draft');
  view.rerender(<TaskHome {...view.props} workspaceId="wsp_other" />);
  click('Write a document');
  fill('Subject', 'Workspace draft');
  view.rerender(<TaskHome {...view.props} sandboxId="sbx_other" />);
  expect(screen.getByLabelText('Your task')).toHaveValue('');
  view.rerender(<TaskHome {...view.props} workspaceId="wsp_other" />);
  expect(screen.getByLabelText('Subject')).toHaveValue('Workspace draft');
  view.rerender(<TaskHome {...view.props} />);
  expect(screen.getByLabelText('Subject')).toHaveValue('Home draft');
  retainTaskViewer('other');
  view.rerender(<TaskHome {...view.props} viewerId="other" />);
  expect(screen.getByLabelText('Your task')).toHaveValue('');
  expect(screen.queryByText('Home draft')).toBeNull();
  expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).toBeNull();
});

it.each([false, true])(
  'retains partial fields when template support is %s without bypassing submit validation',
  async (support) => {
    const { create } = setup({
      readTaskTemplateSupport: async () => support,
      readTaskTemplateV2Support: async () => support,
    });
    await waitFor(() => expect(screen.queryByText('Checking task creation…')).toBeNull());
    click('Write a document');
    fill('Subject', 'Incomplete task');
    click('Write a task without a template');
    click('Switch and keep draft');
    expect(screen.getByLabelText('Your task')).toHaveValue('');
    click('Resume document template');
    const field = screen.getByLabelText('Subject');
    expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
    for (const modifier of ['ctrlKey', 'metaKey'])
      fireEvent.keyDown(field, { key: 'Enter', [modifier]: true });
    fireEvent.submit(field.closest('form')!);
    expect(create).not.toHaveBeenCalled();
    expect(screen.getByLabelText('Document type')).toHaveFocus();
  },
);

it('cleans only the confirmed submitted mode and retains other drafts for the task', () => {
  setup();
  fill('Your task', 'Separate text');
  click('Write a document');
  fill('Subject', 'Keep this partial draft');
  click('Research a topic');
  fill('Topic', 'Submitted topic');
  act(() =>
    discardSubmittedTaskDraft(
      'owner',
      {
        name: 'New task',
        initial_message:
          'Research Submitted topic and summarize the key findings, with links to sources.',
        template: { id: 'research', version: 1, fields: { topic: 'Submitted topic' } },
      },
      draftRevision(),
    ),
  );
  expect(screen.getByLabelText('Your task')).toHaveValue('Separate text');
  expect(screen.queryByRole('button', { name: 'Resume research template' })).toBeNull();
  click('Resume document template');
  expect(screen.getByLabelText('Subject')).toHaveValue('Keep this partial draft');
});

it('retains template drafts when the separate free-text task completes', () => {
  setup();
  fill('Your task', 'Submitted free text');
  click('Write a document');
  fill('Subject', 'Unsubmitted template');
  click('Write a task without a template');
  click('Switch and keep draft');
  act(() =>
    discardSubmittedTaskDraft(
      'owner',
      {
        name: 'New task',
        initial_message: 'Submitted free text',
      },
      draftRevision(),
    ),
  );
  expect(screen.getByLabelText('Your task')).toHaveValue('');
  click('Resume document template');
  expect(screen.getByLabelText('Subject')).toHaveValue('Unsubmitted template');
});

it.each(['text', 'template'] as const)(
  'preserves resumed %s and retained fields when an older identical task completes',
  (mode) => {
    setup();
    fill('Your task', 'Same text');
    click('Write a document');
    fill('Subject', 'Keep this separate draft');
    if (mode === 'template') {
      click('Research a topic');
      fill('Topic', 'Same topic');
    } else {
      click('Write a task without a template');
      click('Switch and keep draft');
    }
    const revision = draftRevision();
    click('Write a document');
    if (mode === 'template') click('Research a topic');
    else {
      click('Write a task without a template');
      click('Switch and keep draft');
    }
    const stored = sessionStorage.getItem(taskDraftStorageKey('owner'));
    act(() =>
      discardSubmittedTaskDraft(
        'owner',
        {
          name: 'New task',
          initial_message:
            mode === 'text'
              ? 'Same text'
              : 'Research the following topic:\n\nSame topic\n\nSummarize the key findings, with links to sources.',
          ...(mode === 'template'
            ? {
                template: {
                  id: 'research' as const,
                  version: 2 as const,
                  fields: { topic: 'Same topic' },
                },
              }
            : {}),
        },
        revision,
      ),
    );
    expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).toBe(stored);
    expect(screen.getByLabelText(mode === 'text' ? 'Your task' : 'Topic')).toHaveValue(
      mode === 'text' ? 'Same text' : 'Same topic',
    );
    click('Write a document');
    expect(screen.getByLabelText('Subject')).toHaveValue('Keep this separate draft');
  },
);

it('keeps template drafts editable through full capacity without creating anything', async () => {
  const { create } = setup({
    readSandboxCapacity: async () => ({ ...AVAILABLE_CAPACITY, available: 0 }),
  });
  await waitFor(() => expect(screen.queryByText('Checking task creation…')).toBeNull());
  click('Write a document');
  fill('Subject', 'Draft while full');
  click('Write code');
  fill('Language', 'Python');
  click('Write a document');
  expect(screen.getByLabelText('Subject')).toHaveValue('Draft while full');
  click('Write a task without a template');
  click('Switch and keep draft');
  click('Resume code template');
  expect(screen.getByLabelText('Language')).toHaveValue('Python');
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  expect(create).not.toHaveBeenCalled();
});

it.each(taskTemplates)(
  'exits an untouched $id template without prompting or losing separate text',
  ({ id, name, fields }) => {
    const view = setup();
    fill('Your task', 'Independent text survives');
    click(name);
    click('Write a task without a template');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.getByLabelText('Your task')).toHaveValue('Independent text survives');
    click(`Resume ${id} template`);
    for (const field of fields) expect(screen.getByLabelText(field.label)).toHaveValue('');
    expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
    expect(view.create).not.toHaveBeenCalled();
  },
);
