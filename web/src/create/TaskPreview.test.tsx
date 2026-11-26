import { selectTaskTemplate } from '../test/taskTemplateSelection';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { unavailableClient, type CmaWebClient } from '../api/client';
import { taskExperience } from '../test/taskExperience';
import { AVAILABLE_CAPACITY } from '../test/capacity';
import { TaskHome } from './TaskHome';

afterEach(() => {
  sessionStorage.clear();
});

async function setup(overrides: Partial<CmaWebClient> = {}) {
  const create = vi.fn().mockRejectedValue(new Error('Unconfirmed request'));
  render(
    <TaskHome
      client={{
        ...unavailableClient,
        taskExperience: taskExperience('owner'),
        readTaskCreationSupport: async () => true,
        readTaskTemplateSupport: async () => true,
        readSandboxCapacity: async () => AVAILABLE_CAPACITY,
        createAgentLifecycle: create,
        ...overrides,
      }}
      viewerId="owner"
      canCreateSandbox
      onCreated={() => {}}
    />,
  );
  await waitFor(() =>
    expect(screen.queryByText('Checking task creation…')).not.toBeInTheDocument(),
  );
  selectTaskTemplate('Write a document');
  await waitFor(() => expect(screen.getByLabelText('Document type')).toHaveFocus());
  return create;
}

function fill(label: string, value: string) {
  fireEvent.change(screen.getByLabelText(label), { target: { value } });
}
function complete() {
  fill('Document type', 'guide');
  fill('Subject', '合成 [data]');
  fill('Audience', 'readers');
}
const previewName = 'Task preview · read-only';
const prompt = 'Fill in the required fields above to generate the task preview.';
const generated = 'Write a guide about 合成 [data] for readers.';

it('visibly distinguishes an empty or partial preview from editable required fields', async () => {
  const create = await setup();
  const preview = screen.getByRole('region', { name: previewName });
  expect(screen.getByRole('heading', { name: previewName })).toBeVisible();
  expect(preview).toHaveTextContent(prompt);
  expect(
    screen.queryByPlaceholderText('Describe the job. It gets its own cloud computer to do it.'),
  ).toBeNull();
  expect(document.querySelector('textarea')).toBeNull();
  expect(screen.queryByText(/↵ to run/)).toBeNull();
  fill('Document type', 'guide');
  fill('Subject', '合成 [data]');
  expect(preview).toHaveTextContent(prompt);
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  fireEvent.keyDown(screen.getByLabelText('Subject'), { key: 'Enter', ctrlKey: true });
  expect(create).not.toHaveBeenCalled();
  expect(screen.getByLabelText('Audience')).toHaveFocus();
});

it('updates complete preview text without moving focus or announcing each edit', async () => {
  const create = await setup();
  screen.getByLabelText('Audience').focus();
  complete();
  const preview = screen.getByRole('region', { name: previewName });
  expect(preview.textContent).toBe(generated);
  expect(preview.closest('[aria-live], [role="status"], [role="alert"]')).toBeNull();
  expect(screen.getByLabelText('Audience')).toHaveFocus();
  expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled();
  expect(screen.getByText(/↵ to run/)).toBeVisible();
  fireEvent.keyDown(screen.getByLabelText('Audience'), { key: 'Enter', ctrlKey: true });
  await waitFor(() => expect(create).toHaveBeenCalledOnce());
  expect(create.mock.calls[0]![0].initial_message).toBe(preview.textContent);
  expect(screen.queryByText(/↵ to run/)).toBeNull();
});

it.each(['ctrlKey', 'metaKey'])(
  'supports %s+Enter while reading a complete preview',
  async (modifier) => {
    const create = await setup();
    complete();
    const preview = screen.getByRole('region', { name: previewName });
    preview.focus();
    fireEvent.keyDown(preview, { key: 'Enter' });
    expect(create).not.toHaveBeenCalled();
    fireEvent.keyDown(preview, { key: 'Enter', [modifier]: true });
    await waitFor(() => expect(create).toHaveBeenCalledOnce());
    expect(create.mock.calls[0]![0].initial_message).toBe(generated);
  },
);

it('keeps complete unsupported templates readable and editable without a runnable shortcut', async () => {
  const create = await setup({ readTaskTemplateSupport: async () => false });
  complete();
  expect(screen.getByRole('region', { name: previewName }).textContent).toBe(generated);
  expect(
    screen.getByText(/Template tasks are (?:not available|unavailable) on this service/),
  ).toBeVisible();
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  expect(screen.queryByText(/↵ to run/)).toBeNull();
  const preview = screen.getByRole('region', { name: previewName });
  expect(preview).not.toHaveAttribute('aria-keyshortcuts');
  fireEvent.keyDown(preview, { key: 'Enter', ctrlKey: true });
  fireEvent.keyDown(screen.getByLabelText('Audience'), { key: 'Enter', metaKey: true });
  expect(create).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Write a task without a template' }));
  expect(screen.queryByRole('region', { name: previewName })).toBeNull();
  expect(screen.getByLabelText('Your task')).toHaveValue(generated);
  fill('Your task', 'Editable free text [1, 2]');
  expect(screen.getByLabelText('Your task')).toHaveValue('Editable free text [1, 2]');
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await waitFor(() => expect(create).toHaveBeenCalledOnce());
  expect(create.mock.calls[0]![0]).toMatchObject({ initial_message: 'Editable free text [1, 2]' });
  expect(create.mock.calls[0]![0]).not.toHaveProperty('template');
});
