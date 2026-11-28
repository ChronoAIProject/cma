import { AVAILABLE_CAPACITY } from '../test/capacity';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { unavailableClient, type CmaWebClient } from '../api/client';
import { taskExperience } from '../test/taskExperience';
import { TaskHome } from './TaskHome';
import { taskDraftStorageKey } from './taskDraft';

afterEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
});

function draw(
  overrides: Partial<CmaWebClient> = {},
  scope: { workspaceId?: string; canCreateSandbox?: boolean } = {},
) {
  const create = vi.fn().mockRejectedValue(new Error('Receipt uncertain'));
  const client = {
    ...unavailableClient,
    readSandboxCapacity: async () => AVAILABLE_CAPACITY,
    taskExperience: taskExperience('owner'),
    readTaskCreationSupport: async () => true,
    readTaskRepositorySupport: async () => true,
    createAgentLifecycle: create,
    ...overrides,
  };
  const view = render(
    <TaskHome client={client} viewerId="owner" canCreateSandbox {...scope} onCreated={() => {}} />,
  );
  return { ...view, create };
}
async function ready() {
  await waitFor(() =>
    expect(screen.queryByText('Checking task creation…')).not.toBeInTheDocument(),
  );
}

it('validates, normalizes, edits and removes a repository while preserving the task draft', async () => {
  const actor = userEvent.setup();
  const { create } = draw();
  await ready();
  await actor.type(screen.getByLabelText('Your task'), 'Preserve this task');
  const trigger = screen.getByRole('button', { name: 'Add git repo' });
  await actor.click(trigger);
  const dialog = screen.getByRole('dialog', { name: 'Add git repo' });
  const repo = within(dialog).getByLabelText('GitHub repository');
  expect(dialog).toHaveAttribute('aria-modal', 'true');
  expect(repo).toHaveFocus();
  const add = within(dialog).getByRole('button', { name: 'Add repo' });
  expect(add).toBeDisabled();
  await actor.type(repo, 'https://www.github.com/owner/repo');
  expect(add).toBeDisabled();
  await actor.clear(repo);
  await actor.type(repo, 'github.com/owner/repo.git/');
  expect(add).toBeEnabled();
  await actor.type(within(dialog).getByLabelText('Repository ref'), 'bad..ref');
  expect(add).toBeDisabled();
  expect(within(dialog).getByRole('alert')).toHaveTextContent('Enter a valid repository ref.');
  await actor.clear(within(dialog).getByLabelText('Repository ref'));
  await actor.type(within(dialog).getByLabelText('Repository ref'), 'feature/demo{Enter}');
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(create).not.toHaveBeenCalled();
  expect(screen.getByText('feature/demo')).toBeVisible();
  const key = taskDraftStorageKey('owner');
  expect(JSON.parse(sessionStorage.getItem(key)!)).toMatchObject({
    repositoryUrl: 'https://github.com/owner/repo',
    repositoryRef: 'feature/demo',
  });
  await actor.click(screen.getByRole('button', { name: 'owner/repo' }));
  expect(screen.getByLabelText('GitHub repository')).toHaveValue('https://github.com/owner/repo');
  expect(screen.getByLabelText('Repository ref')).toHaveValue('feature/demo');
  await actor.keyboard('{Escape}');
  await waitFor(() => expect(screen.getByRole('button', { name: 'owner/repo' })).toHaveFocus());
  await actor.click(screen.getByRole('button', { name: 'Remove repository' }));
  expect(screen.getByLabelText('Your task')).toHaveValue('Preserve this task');
  expect(screen.getByRole('button', { name: 'Add git repo' })).toBeEnabled();
});

it.each([false, true])(
  'keeps unsupported/contextual retained repositories removable: contextual=%s',
  async (contextual) => {
    const workspaceId = contextual ? 'workspace-owned' : undefined;
    sessionStorage.setItem(
      `cma:task-draft:${JSON.stringify([location.origin, 'owner', workspaceId ?? null, null])}`,
      JSON.stringify({
        version: 1,
        text: 'Retained task',
        repositoryUrl: 'https://github.com/owner/repo',
        repositoryRef: '',
      }),
    );
    const { create } = draw({ readTaskRepositorySupport: async () => false }, { workspaceId });
    await ready();
    expect(screen.getByRole('button', { name: 'owner/repo' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'owner/repo' })).toHaveAccessibleDescription(
      contextual ? /inherited/ : /unavailable/,
    );
    expect(screen.getByText('default')).toBeVisible();
    fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
    expect(screen.getByRole('alert')).toHaveTextContent('Repository tasks are unavailable');
    expect(create).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Remove repository' }));
    expect(screen.getByRole('button', { name: 'Add git repo' })).toBeDisabled();
    expect(screen.getByLabelText('Your task')).toHaveValue('Retained task');
  },
);

it.each(['metaKey', 'ctrlKey'])(
  'submits once with %s+Enter, ignoring composing and empty input',
  async (modifier) => {
    let reject!: (error: Error) => void;
    const create = vi.fn(
      () =>
        new Promise<never>((_, fail) => {
          reject = fail;
        }),
    );
    draw({ createAgentLifecycle: create });
    await ready();
    const input = screen.getByLabelText('Your task');
    expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
    fireEvent.change(input, { target: { value: ' \n ' } });
    fireEvent.keyDown(input, { key: 'Enter', [modifier]: true });
    expect(create).not.toHaveBeenCalled();
    fireEvent.change(input, { target: { value: 'Run once' } });
    expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled();
    fireEvent.keyDown(input, { key: 'Enter', [modifier]: true, isComposing: true });
    expect(create).not.toHaveBeenCalled();
    fireEvent.keyDown(input, { key: 'Enter', [modifier]: true });
    fireEvent.keyDown(input, { key: 'Enter', [modifier]: true });
    fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    expect(screen.getByRole('status')).toHaveTextContent('Checking the original request');
    expect(screen.queryByRole('button', { name: 'Start task' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
    await act(async () => reject(new Error('Receipt uncertain')));
    expect(screen.getByRole('status')).toHaveTextContent('Checking the original request');
    expect(screen.getByRole('button', { name: 'Recover original task' })).toBeEnabled();
  },
);

it.each(['queued', 'running', 'failed', 'waiting_for_connection', 'succeeded'])(
  'announces the appropriate accepted %s state',
  async (status) => {
    draw({
      createAgentLifecycle: async () => ({ id: 'operation', status: 'queued' }),
      readAgentLifecycle: async () => ({ id: 'operation', status, phase: 'queued', events: [] }),
    });
    await ready();
    fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'Prepare task' } });
    fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
    const expected =
      status === 'failed'
        ? 'Setup needs attention'
        : status === 'waiting_for_connection'
          ? 'Waiting for connection'
          : status === 'succeeded'
            ? 'Cloud computer ready'
            : 'Preparing';
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent(expected));
    expect(screen.getByText('Request accepted')).toBeVisible();
    expect(screen.getAllByRole('status')).toHaveLength(1);
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
    expect(screen.getByRole('region', { name: 'Agent provisioning progress' })).toBeVisible();
    expect(screen.getByText('Setup details').closest('details')).not.toHaveAttribute('open');
  },
);

it('uses the shared keyboard menu, exposes the sole radio, and returns focus on dismissal', async () => {
  const actor = userEvent.setup();
  draw();
  await ready();
  const trigger = screen.getByRole('button', { name: /^Cloud computer/ });
  await actor.click(trigger);
  expect(screen.getByRole('menuitemradio', { name: 'Cloud computer' })).toBeChecked();
  expect(screen.getAllByRole('menuitemradio')).toHaveLength(1);
  expect(screen.getByRole('menuitemradio')).toHaveFocus();
  await actor.keyboard('{ArrowDown}');
  expect(screen.getByRole('menuitemcheckbox', { name: 'Shared desktop' })).toHaveFocus();
  expect(screen.queryByRole('menuitemcheckbox', { name: 'Agent browser' })).not.toBeInTheDocument();
  await actor.keyboard('{Enter}');
  expect(screen.getByRole('menuitemcheckbox', { name: 'Shared desktop' })).toBeChecked();
  expect(trigger).toHaveTextContent('Shared desktop');
  await actor.keyboard('{Escape}');
  await waitFor(() => expect(trigger).toHaveFocus());
  expect(trigger).toHaveAttribute('aria-expanded', 'false');
  await actor.click(trigger);
  fireEvent.pointerDown(document.body);
  await waitFor(() => expect(trigger).toHaveAttribute('aria-expanded', 'false'));
});

it.each(['unsupported', 'permission'])(
  'keeps nonempty task admission disabled for %s',
  async (gate) => {
    const { create } = draw(
      { readTaskCreationSupport: async () => gate !== 'unsupported' },
      { canCreateSandbox: gate !== 'permission' },
    );
    await ready();
    const input = screen.getByLabelText('Your task');
    fireEvent.change(input, { target: { value: 'Task with content' } });
    expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
    fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true });
    expect(create).not.toHaveBeenCalled();
  },
);

it.each([true, false])(
  'announces repository checking until support resolves to %s',
  async (supported) => {
    let resolve!: (value: boolean) => void;
    const { create } = draw({
      readTaskRepositorySupport: () =>
        new Promise<boolean>((done) => {
          resolve = done;
        }),
    });
    await ready();
    const add = screen.getByRole('button', { name: 'Add git repo' });
    expect(add).toBeDisabled();
    expect(add).toHaveAccessibleDescription('Checking repository support…');
    expect(
      screen.queryByText('Repository tasks are unavailable on this service.'),
    ).not.toBeInTheDocument();
    await act(async () => resolve(supported));
    if (supported) expect(add).toBeEnabled();
    else {
      expect(add).toBeDisabled();
      expect(add).toHaveAccessibleDescription('Repository tasks are unavailable on this service.');
    }
    expect(create).not.toHaveBeenCalled();
  },
);

it('does not call a pending repository check unsupported when submitting a retained draft', async () => {
  sessionStorage.setItem(
    `cma:task-draft:${JSON.stringify([location.origin, 'owner', null, null])}`,
    JSON.stringify({
      version: 1,
      text: 'Retained repository task',
      repositoryUrl: 'https://github.com/owner/repo',
    }),
  );
  let resolve!: (value: boolean) => void;
  let resolveCapacity!: (value: typeof AVAILABLE_CAPACITY) => void;
  const { create } = draw({
    readSandboxCapacity: () =>
      new Promise<typeof AVAILABLE_CAPACITY>((done) => {
        resolveCapacity = done;
      }),
    readTaskRepositorySupport: () =>
      new Promise<boolean>((done) => {
        resolve = done;
      }),
  });
  await ready();
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  await act(async () => resolveCapacity(AVAILABLE_CAPACITY));
  expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled();
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  expect(screen.getByRole('alert')).toHaveTextContent(
    'Checking repository support. Wait for the check to finish before starting.',
  );
  expect(create).not.toHaveBeenCalled();
  await act(async () => resolve(false));
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  expect(screen.getByRole('alert')).toHaveTextContent('Repository tasks are unavailable');
  expect(create).not.toHaveBeenCalled();
});

it('treats an absent repository support reader as unsupported immediately', () => {
  draw({ readTaskRepositorySupport: undefined });
  expect(screen.getByRole('button', { name: 'Add git repo' })).toHaveAccessibleDescription(
    'Repository tasks are unavailable on this service.',
  );
  expect(screen.queryByText('Checking repository support…')).not.toBeInTheDocument();
});

it('explains invalid and empty repository input on blur or Enter without native validation', async () => {
  const actor = userEvent.setup();
  draw();
  await ready();
  await actor.click(screen.getByRole('button', { name: 'Add git repo' }));
  const repo = screen.getByLabelText('GitHub repository');
  expect(repo).toHaveAttribute('aria-required', 'true');
  expect(repo).not.toHaveAttribute('required');
  expect(repo.closest('form')).toHaveAttribute('novalidate');
  await actor.type(repo, 'not-a-repository');
  expect(repo).toHaveAttribute('aria-invalid', 'false');
  await actor.tab();
  expect(repo).toHaveAttribute('aria-invalid', 'true');
  expect(repo).toHaveAccessibleDescription(
    'Enter owner/repository or a github.com repository URL.',
  );
  await actor.clear(repo);
  await actor.type(repo, '{Enter}');
  expect(screen.getByRole('alert')).toHaveTextContent(
    'Enter owner/repository or a github.com repository URL.',
  );
  await actor.type(repo, 'owner/repo');
  expect(repo).toHaveAttribute('aria-invalid', 'false');
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Add repo' })).toBeEnabled();
});

it('keeps contextual task entry compact and describes the existing Sandbox truthfully', async () => {
  draw(
    { createAgentLifecycle: async () => ({ id: 'contextual-operation', status: 'queued' }) },
    { workspaceId: 'workspace-owned' },
  );
  await ready();
  expect(
    screen.queryByRole('heading', { name: 'What should your Agent work on?' }),
  ).not.toBeInTheDocument();
  expect(screen.queryByText('New agent', { exact: true })).not.toBeInTheDocument();
  expect(screen.getByText('Creating in Workspace workspace-owned')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: /^Cloud computer/ }));
  expect(screen.getByRole('menuitemradio', { name: 'Cloud computer' })).toHaveAccessibleDescription(
    "This Sandbox's existing Linux machine",
  );
  fireEvent.keyDown(document, { key: 'Escape' });
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'Use this workspace' } });
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Preparing'));
  expect(screen.getByRole('region', { name: 'Original request' })).toHaveTextContent(
    'Use this workspace',
  );
  expect(screen.getByText('Acceptance time unavailable')).toBeVisible();
});

it.each([
  ['Macintosh', 'Meta+Enter'],
  ['Linux', 'Control+Enter'],
])('exposes the task shortcut on %s', (userAgent, shortcut) => {
  vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue(userAgent);
  draw();
  expect(screen.getByLabelText('Your task')).toHaveAttribute('aria-keyshortcuts', shortcut);
});
