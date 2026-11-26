import { selectTaskTemplate } from '../test/taskTemplateSelection';
import {
  act,
  fireEvent,
  render,
  renderHook,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { unavailableClient } from '../api/client';
import { AVAILABLE_CAPACITY } from '../test/capacity';
import { taskExperience } from '../test/taskExperience';
import { TaskHome } from './TaskHome';
import { useTaskDraft, taskDraftStorageKey } from './taskDraft';
import { useTaskDraft as usePredecessorDraft } from './taskDraft.v2.fixture';

const exactFields = { type: ' report ', subject: '  草稿 [subject]  ', audience: '' };
afterEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
});
function seedCompetingDrafts() {
  const current = renderHook(() => useTaskDraft('owner'));
  act(() => current.result.current.change('Current free text'));
  current.unmount();
  const old = renderHook(() => usePredecessorDraft('owner'));
  act(() => old.result.current.changeTemplate({ id: 'document', version: 1, fields: exactFields }));
  act(() =>
    old.result.current.changeRepository('https://github.com/owner/project', 'feature/draft'),
  );
  act(() => old.result.current.changeMachineIncluded(true));
  old.unmount();
}
function setup(support = true) {
  const create = vi.fn();
  const props = {
    client: {
      ...unavailableClient,
      taskExperience: taskExperience('owner'),
      readTaskCreationSupport: async () => true,
      readTaskTemplateSupport: async () => support,
      readTaskRepositorySupport: async () => true,
      readSandboxCapacity: async () => AVAILABLE_CAPACITY,
      createAgentLifecycle: create,
    },
    viewerId: 'owner',
    canCreateSandbox: true,
    onCreated: vi.fn(),
  };
  return { ...render(<TaskHome {...props} />), props, create };
}
const restore = () => fireEvent.click(screen.getByRole('button', { name: 'Use this saved draft' }));
const showRecovery = () => fireEvent.click(screen.getByText(/Other saved task drafts/));

it.each([true, false])(
  'restores exact saved fields and options, keeps the displaced draft and enforces template support %s',
  async (support) => {
    seedCompetingDrafts();
    const view = setup(support);
    showRecovery();
    expect(screen.getByLabelText('Saved draft 1 details')).toHaveValue(
      'Write a document\nDocument type:  report \nSubject:   草稿 [subject]  \nAudience: \n\nRepository: https://github.com/owner/project\n\nRef: feature/draft\n\nShared desktop: included',
    );
    restore();
    await waitFor(() => expect(screen.getByLabelText('Document type')).toHaveFocus());
    expect(screen.getByLabelText('Subject')).toHaveValue(exactFields.subject);
    expect(screen.getByLabelText('Document type')).toHaveValue(exactFields.type);
    expect(screen.getByLabelText('Audience')).toHaveValue('');
    expect(screen.getByText('feature/draft')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
    fireEvent.submit(screen.getByLabelText('Subject').closest('form')!);
    expect(view.create).not.toHaveBeenCalled();
    if (!support) {
      fireEvent.change(screen.getByLabelText('Audience'), { target: { value: 'readers' } });
      expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
      fireEvent.submit(screen.getByLabelText('Audience').closest('form')!);
      expect(view.create).not.toHaveBeenCalled();
    }
    view.unmount();
    render(<TaskHome {...view.props} />);
    expect(screen.getByLabelText('Subject')).toHaveValue(exactFields.subject);
    showRecovery();
    expect(screen.getByLabelText('Saved draft 1 details')).toHaveValue(
      'Task: Current free text\n\nShared desktop: not included',
    );
    restore();
    await waitFor(() => expect(screen.getByLabelText('Your task')).toHaveFocus());
    expect(screen.getByLabelText('Your task')).toHaveValue('Current free text');
    expect(screen.queryByRole('button', { name: 'Remove repository' })).toBeNull();
    expect(screen.getByLabelText<HTMLTextAreaElement>('Saved draft 1 details').value).toContain(
      exactFields.subject,
    );
    expect(view.create).not.toHaveBeenCalled();
  },
);

it('keeps both drafts and the chosen button focused after a refused recovery write', async () => {
  seedCompetingDrafts();
  const { create } = setup();
  showRecovery();
  const saved = sessionStorage.getItem(taskDraftStorageKey('owner'));
  const deny = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new Error('Full');
  });
  const button = within(screen.getByRole('region', { name: 'Saved draft 1' })).getByRole('button');
  button.focus();
  fireEvent.click(button);
  expect(button).toHaveFocus();
  expect(screen.getByLabelText('Your task')).toHaveValue('Current free text');
  expect(screen.getByRole('alert')).toHaveTextContent('could not be saved');
  expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).toBe(saved);
  expect(screen.getByLabelText<HTMLTextAreaElement>('Saved draft 1 details').value).toContain(
    exactFields.subject,
  );
  expect(create).not.toHaveBeenCalled();
  deny.mockRestore();
  fireEvent.click(button);
  await waitFor(() => expect(screen.getByLabelText('Document type')).toHaveFocus());
  expect(screen.getByLabelText('Subject')).toHaveValue(exactFields.subject);
});

it.each(['template', 'recovery'])(
  'does not steal focus from an edited field after %s selection',
  (mode) => {
    if (mode === 'recovery') seedCompetingDrafts();
    const callbacks: FrameRequestCallback[] = [];
    vi.spyOn(window, 'requestAnimationFrame').mockImplementation((callback) => {
      callbacks.push(callback);
      return callbacks.length;
    });
    const { create } = setup();
    if (mode === 'recovery') {
      showRecovery();
      restore();
    } else selectTaskTemplate('Write a document');
    const subject = screen.getByLabelText('Subject');
    subject.focus();
    fireEvent.change(subject, { target: { value: 'User chose this field' } });
    act(() => {
      for (const callback of callbacks) callback(0);
    });
    expect(subject).toHaveFocus();
    expect(subject).toHaveValue('User chose this field');
    expect(screen.getByLabelText('Document type')).toHaveValue(
      mode === 'recovery' ? exactFields.type : '',
    );
    expect(create).not.toHaveBeenCalled();
  },
);
