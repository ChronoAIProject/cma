import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { SandboxCapacity } from '@cma/client';
import type { CmaWebClient } from '../api/client';
import { agentClient } from '../test/agent';
import { navigationFixture } from '../test/hierarchy';
import { taskExperience } from '../test/taskExperience';
import { TaskHome } from './TaskHome';
import { TaskHomeLayout } from './TaskHomeLayout';
import { TaskCreationStatus } from './TaskCreationStatus';

const capacity: SandboxCapacity = {
  resource: 'sandbox',
  limit: 5,
  live: 2,
  available: 3,
  counts_by_status: { active: 2 },
  observed_at: '2026-11-22T06:00:00Z',
};
const full: SandboxCapacity = {
  ...capacity,
  live: 5,
  available: 0,
  counts_by_status: { active: 3, retiring: 2 },
};
function client(overrides: Partial<CmaWebClient> = {}) {
  return agentClient({
    taskExperience: taskExperience('owner'),
    readTaskCreationSupport: async () => true,
    readTaskTitleModeSupport: async () => true,
    readTaskTemplateSupport: async () => true,
    readSandboxCapacity: async () => capacity,
    ...overrides,
  });
}
afterEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
});

it('uses neutral empty-state copy only after an empty personal read succeeds, never across scopes', () => {
  const props = {
    titleId: 'home-title',
    contextual: false,
    submitted: false,
    onNavigate: vi.fn(),
    children: <p>Composer</p>,
  };
  const { rerender } = render(
    <TaskHomeLayout {...props} home={{ navigation: { status: 'loading' } }} />,
  );
  expect(screen.queryByText('Your next agent starts here')).not.toBeInTheDocument();
  rerender(
    <TaskHomeLayout
      {...props}
      home={{ navigation: { status: 'error', message: 'Access unavailable' } }}
    />,
  );
  expect(screen.getByRole('alert')).toHaveTextContent('Access unavailable');
  expect(screen.queryByText('Your next agent starts here')).not.toBeInTheDocument();
  rerender(
    <TaskHomeLayout
      {...props}
      home={{ navigation: { status: 'ready', items: [], refreshError: 'Stale list' } }}
    />,
  );
  expect(screen.queryByText('Your next agent starts here')).not.toBeInTheDocument();
  rerender(<TaskHomeLayout {...props} home={{ navigation: { status: 'ready', items: [] } }} />);
  expect(screen.getByRole('heading', { name: 'Give it a job.' })).toBeVisible();
  expect(screen.getByText('Your next agent starts here')).toBeVisible();
  expect(screen.queryByText('Your first agent starts here')).not.toBeInTheDocument();
  rerender(
    <TaskHomeLayout
      {...props}
      home={{ navigation: { status: 'ready', items: navigationFixture() }, scope: 'shared' }}
    />,
  );
  expect(screen.queryByText('Agent one')).not.toBeInTheDocument();
  expect(screen.queryByText('Your next agent starts here')).not.toBeInTheDocument();
});

it.each([
  [3, undefined],
  [1, '1 environment slot left'],
  [0, 'All 5 Sandbox environment slots are in use'],
] as const)(
  'presents %s free slots from capacity independently of the crew count',
  async (available, message) => {
    render(
      <TaskHome
        client={client({
          readSandboxCapacity: async () => ({ ...capacity, available, live: 5 - available }),
        })}
        viewerId="owner"
        canCreateSandbox
        onCreated={vi.fn()}
        home={{ navigation: { status: 'ready', items: [] } }}
      />,
    );
    fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'Keep this draft' } });
    await waitFor(() =>
      expect(screen.queryByText('Checking Sandbox capacity…')).not.toBeInTheDocument(),
    );
    if (message) expect(screen.getByText(message)).toBeVisible();
    else expect(screen.queryByRole('region', { name: 'Sandbox capacity' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Start task' }).hasAttribute('disabled')).toBe(
      available === 0,
    );
  },
);

it('replaces stale full capacity with unknown, coalesces a deliberate refresh and keeps the draft', async () => {
  let settle!: (value: SandboxCapacity) => void;
  const read = vi
    .fn<CmaWebClient['readSandboxCapacity'] & NonNullable<CmaWebClient['readSandboxCapacity']>>()
    .mockResolvedValueOnce(full)
    .mockRejectedValueOnce(new Error('Unavailable'))
    .mockImplementation(
      () =>
        new Promise((resolve) => {
          settle = resolve;
        }),
    );
  const create = vi.fn();
  render(
    <TaskHome
      client={client({ readSandboxCapacity: read, createAgentLifecycle: create })}
      viewerId="owner"
      canCreateSandbox
      onCreated={vi.fn()}
    />,
  );
  await screen.findByText('All 5 Sandbox environment slots are in use');
  fireEvent.change(screen.getByLabelText('Your task'), {
    target: { value: 'Draft survives uncertainty' },
  });
  fireEvent.focus(window);
  await screen.findByText('Capacity could not be checked');
  expect(screen.queryByText('All 5 Sandbox environment slots are in use')).not.toBeInTheDocument();
  const retry = screen.getByRole('button', { name: 'Check again' });
  fireEvent.click(retry);
  expect(retry).toBeDisabled();
  fireEvent.click(retry);
  await waitFor(() => expect(read).toHaveBeenCalledTimes(3));
  await act(async () => {
    settle(capacity);
  });
  expect(screen.getByLabelText('Your task')).toHaveValue('Draft survives uncertainty');
  expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled();
  expect(create).not.toHaveBeenCalled();
});

it('preserves template fields on a capability read failure without enabling submission', async () => {
  const create = vi.fn();
  render(
    <TaskHome
      client={client({
        readTaskTemplateSupport: async () => {
          throw new Error('Read failed');
        },
        createAgentLifecycle: create,
      })}
      viewerId="owner"
      canCreateSandbox
      onCreated={vi.fn()}
    />,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Research a topic' }));
  fireEvent.change(screen.getByLabelText('Topic'), { target: { value: '研究 сохраняется' } });
  await screen.findByText('Template availability could not be checked.');
  expect(screen.getByLabelText('Topic')).toHaveValue('研究 сохраняется');
  expect(screen.getByRole('region', { name: 'Task preview · read-only' })).toHaveTextContent(
    'Research 研究 сохраняется',
  );
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  expect(create).not.toHaveBeenCalled();
});

it('shows an accepted request and stale preparation as separate facts with only a real event time', () => {
  const intent = {
    key: 'synthetic-retained-key',
    body: { name: 'New task', initial_message: 'Keep the original message' },
    receipt: { id: 'operation', status: 'queued' },
  };
  const { rerender } = render(
    <TaskCreationStatus
      intent={intent}
      pending={false}
      preparing={false}
      failure="Status unavailable"
      operation={{
        id: 'operation',
        status: 'running',
        phase: 'sandbox_runtime',
        events: [
          {
            sequence: 1,
            phase: 'sandbox_ready',
            status: 'running',
            message: 'Preparing',
            occurred_at: '2026-11-22T06:00:00Z',
          },
        ],
      }}
    />,
  );
  expect(screen.getByRole('region', { name: 'Submitted task' })).toHaveTextContent(
    'Keep the original message',
  );
  expect(screen.getByText(/Last confirmed: Preparing/)).toBeVisible();
  expect(screen.getByRole('alert')).toHaveTextContent('Status unavailable');
  expect(document.querySelector('time')).toHaveAttribute('datetime', '2026-11-22T06:00:00Z');
  rerender(
    <TaskCreationStatus
      intent={intent}
      pending={false}
      preparing={false}
      failure="Status unavailable"
    />,
  );
  expect(screen.getByText(/Last confirmed: Request accepted/)).toBeVisible();
  expect(document.querySelector('time')).toBeNull();
});

it('uses neutral copy for returning owners whose earlier workspaces were removed', () => {
  const items = navigationFixture().map((item) => ({
    ...item,
    workspaces: item.workspaces.map((workspace) => ({ ...workspace, status: 'removed' as const })),
  }));
  render(
    <TaskHomeLayout
      titleId="returning-owner"
      contextual={false}
      submitted={false}
      onNavigate={vi.fn()}
      home={{ navigation: { status: 'ready', items } }}
    >
      Composer
    </TaskHomeLayout>,
  );
  expect(screen.getByText('Your next agent starts here')).toBeVisible();
  expect(screen.queryByText('Your first agent starts here')).not.toBeInTheDocument();
});
