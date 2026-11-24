import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { unavailableClient, type CmaWebClient } from '../api/client';
import { AVAILABLE_CAPACITY } from '../test/capacity';
import { taskExperience } from '../test/taskExperience';
import { TaskHome, type TaskHomeProps } from './TaskHome';
import { TaskTemplateFields } from './TaskTemplateFields';
import { compileTaskTemplate, type TaskTemplate } from './taskTemplateModel';
import { lifecycleStorageKey } from './lifecycleIntent';
import { taskDraftStorageKey } from './taskDraft';

const ERROR = 'Enter a complete HTTP or HTTPS link to your spreadsheet.';
const VALID_LINK = ' https://example.com/synthetic.csv ';
const spreadsheet = (link: string): TaskTemplate => ({
  id: 'spreadsheet',
  version: 1,
  fields: { link },
});

afterEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
});

function setup(context: Partial<TaskHomeProps> = {}, overrides: Partial<CmaWebClient> = {}) {
  const create = vi.fn().mockRejectedValue(new TypeError('Response lost'));
  const capacity = vi.fn().mockResolvedValue(AVAILABLE_CAPACITY);
  const props = {
    client: {
      ...unavailableClient,
      taskExperience: taskExperience('owner'),
      readTaskCreationSupport: async () => true,
      readTaskTemplateSupport: async () => true,
      readSandboxCapacity: capacity,
      createAgentLifecycle: create,
      ...overrides,
    },
    viewerId: 'owner',
    canCreateSandbox: true,
    onCreated: vi.fn(),
    ...context,
  };
  return { ...render(<TaskHome {...props} />), props, create, capacity };
}

async function chooseTemplate(name: string) {
  const button = screen.queryByRole('button', { name });
  if (button) {
    fireEvent.click(button);
    return;
  }
  fireEvent.click(screen.getByRole('button', { name: 'Templates' }));
  fireEvent.click(await screen.findByRole('menuitem', { name }));
}

async function chooseSpreadsheet() {
  await waitFor(() => expect(screen.queryByText('Checking task creation…')).toBeNull());
  await chooseTemplate('Analyze a spreadsheet');
  return screen.getByLabelText('Spreadsheet link');
}

it.each([{}, { sandboxId: 'sandbox-existing' }, { workspaceId: 'workspace-existing' }])(
  'refuses an invalid spreadsheet at the actual fresh submit boundary: %j',
  async (context) => {
    const { create, capacity } = setup(context);
    const input = await chooseSpreadsheet();
    fireEvent.change(input, { target: { value: 'not-a-url' } });
    const capacityReads = capacity.mock.calls.length;
    await act(async () => {
      fireEvent.submit(input.closest('form')!);
    });
    expect(create).not.toHaveBeenCalled();
    expect(capacity).toHaveBeenCalledTimes(capacityReads);
    expect(input).toHaveFocus();
    expect(input).toHaveAttribute('aria-invalid', 'true');
    expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  },
);

it('updates one accessible error through correction and blocks every fresh form entry', async () => {
  const { create, capacity } = setup();
  const input = await chooseSpreadsheet();
  const start = screen.getByRole('button', { name: 'Start task' });
  const form = input.closest('form')!;
  fireEvent.change(input, { target: { value: 'not-a-url' } });
  expect(input).toHaveAccessibleDescription(ERROR);
  expect(screen.getAllByText(ERROR)).toHaveLength(1);
  const reads = capacity.mock.calls.length;
  await act(async () => {
    fireEvent.click(start);
    fireEvent.keyDown(input, { key: 'Enter' });
    fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true });
    fireEvent.keyDown(input, { key: 'Enter', metaKey: true });
    form.requestSubmit();
    fireEvent.submit(form);
  });
  expect(create).not.toHaveBeenCalled();
  expect(capacity).toHaveBeenCalledTimes(reads);
  expect(input).toHaveAccessibleDescription(ERROR);
  expect(screen.getAllByText(ERROR)).toHaveLength(1);
  expect(input).toHaveFocus();
  fireEvent.change(input, { target: { value: VALID_LINK } });
  expect(start).toBeEnabled();
  expect(input).not.toHaveAttribute('aria-invalid');
  expect(input).not.toHaveAttribute('aria-describedby');
  expect(screen.queryByText(ERROR)).toBeNull();
  fireEvent.change(input, { target: { value: 'https:///example.com' } });
  expect(start).toBeDisabled();
  expect(input).toHaveAccessibleDescription(ERROR);
  expect(create).not.toHaveBeenCalled();
});

it.each([{}, { sandboxId: 'sandbox-existing' }, { workspaceId: 'workspace-existing' }])(
  'sends the exact valid typed spreadsheet request in context %j',
  async (context) => {
    const { create } = setup(context);
    const input = await chooseSpreadsheet();
    fireEvent.change(input, { target: { value: VALID_LINK } });
    fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
    await screen.findByRole('button', { name: 'Recover original task' });
    const template = spreadsheet(VALID_LINK);
    expect(create).toHaveBeenCalledExactlyOnceWith(
      {
        name: 'New task',
        template,
        initial_message: compileTaskTemplate(template).text,
        ...(context.sandboxId ? { sandbox_id: context.sandboxId } : {}),
        ...(context.workspaceId ? { workspace_id: context.workspaceId } : {}),
      },
      { idempotencyKey: expect.any(String), signal: expect.any(AbortSignal) },
    );
  },
);

it('keeps invalid drafts independently through template switches, reload and correction', async () => {
  const view = setup();
  const input = await chooseSpreadsheet();
  fireEvent.change(input, { target: { value: 'not-a-url' } });
  await chooseTemplate('Write a document');
  fireEvent.change(screen.getByLabelText('Subject'), {
    target: { value: 'Separate document draft' },
  });
  await chooseTemplate('Analyze a spreadsheet');
  expect(screen.getByLabelText('Spreadsheet link')).toHaveValue('not-a-url');
  view.unmount();
  render(<TaskHome {...view.props} />);
  await waitFor(() => expect(screen.queryByText('Checking task creation…')).toBeNull());
  expect(screen.getByLabelText('Spreadsheet link')).toHaveValue('not-a-url');
  expect(screen.getByLabelText('Spreadsheet link')).toHaveAccessibleDescription(ERROR);
  await chooseTemplate('Write a document');
  expect(screen.getByLabelText('Subject')).toHaveValue('Separate document draft');
  await chooseTemplate('Analyze a spreadsheet');
  fireEvent.change(screen.getByLabelText('Spreadsheet link'), { target: { value: VALID_LINK } });
  expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled();
  expect(view.create).not.toHaveBeenCalled();
});

it('recovers a retained invalid-link request after reload with the original body and key', async () => {
  const template = spreadsheet('not-a-url');
  const body = { name: 'New task', template, initial_message: compileTaskTemplate(template).text };
  const key = 'retained-spreadsheet';
  const saved = JSON.stringify({ version: 1, intent: { key, body } });
  sessionStorage.setItem(lifecycleStorageKey('owner'), saved);
  const view = setup();
  await screen.findByRole('button', { name: 'Recover original task' });
  expect(view.create).not.toHaveBeenCalled();
  view.unmount();
  render(<TaskHome {...view.props} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Recover original task' }));
  await waitFor(() => expect(view.create).toHaveBeenCalledOnce());
  expect(view.create).toHaveBeenCalledWith(body, {
    idempotencyKey: key,
    signal: expect.any(AbortSignal),
  });
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toBe(saved);
});

it('isolates spreadsheet drafts on owner/client changes without changing saved requests', async () => {
  const view = setup();
  const input = await chooseSpreadsheet();
  fireEvent.change(input, { target: { value: 'not-a-url' } });
  const originalDraft = sessionStorage.getItem(taskDraftStorageKey('owner'));
  view.rerender(<TaskHome {...view.props} viewerId="another-owner" />);
  expect(screen.queryByLabelText('Spreadsheet link')).toBeNull();
  expect(screen.getByLabelText('Your task')).toHaveValue('');
  expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).toBe(originalDraft);
  view.rerender(
    <TaskHome
      {...view.props}
      client={{
        ...view.props.client,
        readTaskTemplateSupport: () => new Promise<boolean>(() => {}),
      }}
    />,
  );
  const restored = screen.getByLabelText('Spreadsheet link');
  expect(restored).toHaveValue('not-a-url');
  await act(async () => {
    fireEvent.submit(restored.closest('form')!);
  });
  expect(view.create).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).toBe(originalDraft);
});

it('uses unique local error IDs and preserves incoming field errors', () => {
  const template = spreadsheet('not-a-url');
  const props = { template, onChange: vi.fn() };
  const view = render(
    <>
      <TaskTemplateFields {...props} errorId="existing-error" />
      <TaskTemplateFields {...props} errorId="other-error" />
    </>,
  );
  const fields = screen.getAllByLabelText('Spreadsheet link');
  expect(fields[0]).toHaveAccessibleDescription(ERROR);
  expect(fields[1]).toHaveAccessibleDescription(ERROR);
  expect(fields[0]!.getAttribute('aria-describedby')).not.toBe(
    fields[1]!.getAttribute('aria-describedby'),
  );
  view.rerender(
    <>
      <TaskTemplateFields
        {...props}
        errorId="existing-error"
        issue={{ field: 'link', message: 'Existing refusal' }}
      />
      <p id="existing-error" role="alert">
        Existing refusal
      </p>
    </>,
  );
  expect(screen.queryByText(ERROR)).toBeNull();
  expect(screen.getByLabelText('Spreadsheet link')).toHaveAccessibleDescription('Existing refusal');
  view.rerender(
    <>
      <TaskTemplateFields
        {...props}
        errorId="general-error"
        issue={{ message: 'Creation unavailable' }}
      />
      <p id="general-error" role="alert">
        Creation unavailable
      </p>
    </>,
  );
  expect(screen.getByLabelText('Spreadsheet link')).toHaveAccessibleDescription(ERROR);
  expect(screen.getByText('Creation unavailable')).toBeVisible();
});
