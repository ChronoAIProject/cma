import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AuthContextProvider } from '../auth/context';
import { WIRE_AGENT, WIRE_SANDBOX_DETAIL } from '../test/rest';
import { taskExperience } from '../test/taskExperience';
import { TaskHome } from './TaskHome';
import { AuthenticatedTask, identity, taskClient } from './taskHomeTestSupport';

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

it.each([
  { initial_message: { damaged: true } },
  { initial_message: 'Retained task', machine_included: { damaged: true } },
  { initial_message: 'Retained task', repository: { url: { damaged: true }, ref: 'main' } },
  {
    initial_message: 'Retained task',
    repository: { url: 'https://github.com/example/project', ref: [] },
  },
])(
  'keeps a damaged retained task unavailable without crashing or sending a replacement: %j',
  async (damaged) => {
    sessionStorage.setItem(
      `cma:agent-creation:${JSON.stringify([location.origin, 'owner'])}`,
      JSON.stringify({
        version: 1,
        intent: {
          key: 'retained-task',
          body: {
            name: 'New task',
            ...damaged,
          },
        },
      }),
    );
    const transport = vi.fn<typeof fetch>();
    render(
      <TaskHome
        client={taskClient(transport)}
        viewerId="owner"
        canCreateSandbox
        onCreated={() => {}}
      />,
    );
    expect(await screen.findByText(/The saved Agent request could not be read/)).toBeVisible();
    fireEvent.change(screen.getByLabelText('Your task'), {
      target: { value: 'Keep the next draft' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
    expect(transport).not.toHaveBeenCalled();
    expect(screen.getByLabelText('Your task')).toHaveValue('Keep the next draft');
  },
);

it('keeps authenticated task text copyable and sends nothing when session storage is unavailable', async () => {
  const storage = sessionStorage;
  vi.stubGlobal('sessionStorage', {
    getItem: storage.getItem.bind(storage),
    removeItem: storage.removeItem.bind(storage),
    clear: storage.clear.bind(storage),
    key: storage.key.bind(storage),
    length: storage.length,
    setItem: () => {
      throw new DOMException('Storage unavailable');
    },
  });
  const transport = vi.fn<typeof fetch>();
  const client = taskClient(transport);
  const view = render(
    <AuthContextProvider provider={identity('owner')}>
      <AuthenticatedTask client={client} />
    </AuthContextProvider>,
  );
  const input = await screen.findByLabelText('Your task');
  fireEvent.change(input, { target: { value: 'Keep this exact task available to copy' } });
  await waitFor(() =>
    expect(screen.queryByText('Checking task creation…')).not.toBeInTheDocument(),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  expect(input).toHaveValue('Keep this exact task available to copy');
  expect(screen.getByText(/Copy your text and restore browser session storage/)).toBeVisible();
  expect(transport).not.toHaveBeenCalled();
  view.unmount();
  vi.unstubAllGlobals();
  render(
    <AuthContextProvider provider={identity('owner')}>
      <AuthenticatedTask client={client} />
    </AuthContextProvider>,
  );
  expect(await screen.findByLabelText('Your task')).toHaveValue('');
});

it('restores repository drafts and clears only their optional context before starting an empty workspace', async () => {
  const writes: unknown[] = [];
  const client = taskClient(async (_, options) => {
    if (options?.method === 'POST') writes.push(JSON.parse(String(options.body)));
    throw new Error('Lost response');
  });
  const props = { client, viewerId: 'owner', canCreateSandbox: true, onCreated: vi.fn() };
  const first = render(<TaskHome {...props} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Add git repo' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Add git repo' }));
  fireEvent.change(screen.getByLabelText('Your task'), {
    target: { value: 'Keep this exact draft\n ' },
  });
  fireEvent.change(screen.getByLabelText('GitHub repository'), {
    target: { value: 'https://github.com/example/project' },
  });
  fireEvent.change(screen.getByLabelText('Repository ref'), { target: { value: 'proof/fixed' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add repo' }));
  first.unmount();
  render(<TaskHome {...props} />);
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'example/project' })).toBeEnabled(),
  );
  fireEvent.click(screen.getByRole('button', { name: 'example/project' }));
  expect(await screen.findByLabelText('GitHub repository')).toHaveValue(
    'https://github.com/example/project',
  );
  expect(screen.getByLabelText('Repository ref')).toHaveValue('proof/fixed');
  expect(screen.getByLabelText('Your task')).toHaveValue('Keep this exact draft\n ');
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  fireEvent.click(screen.getByRole('button', { name: 'Remove repository' }));
  expect(screen.getByLabelText('Your task')).toHaveValue('Keep this exact draft\n ');
  expect(screen.queryByRole('button', { name: 'Remove repository' })).not.toBeInTheDocument();
  await waitFor(() =>
    expect(screen.queryByText('Checking task creation…')).not.toBeInTheDocument(),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await screen.findByRole('button', { name: 'Recover original task' });
  expect(writes).toEqual([{ name: 'New task', initial_message: 'Keep this exact draft\n ' }]);
});

it('restores an accepted home task after refresh and opens its exact preparing Agent', async () => {
  let admitted = false;
  let readable = false;
  const requests: { body: unknown; key: string | null }[] = [];
  const transport = vi.fn<typeof fetch>(async (input, options) => {
    const path = new URL(String(input), location.origin).pathname;
    if (path === '/api/agent-manager/v1/agents' && options?.method === 'POST') {
      requests.push({
        body: JSON.parse(String(options.body)),
        key: new Headers(options.headers).get('Idempotency-Key'),
      });
      admitted = true;
      return Response.json({ id: 'operation-task', status: 'provisioning' }, { status: 202 });
    }
    if (path.endsWith('/agent-lifecycle/operation-task') && admitted)
      return Response.json({
        id: 'operation-task',
        status: 'running',
        phase: 'agent_startup',
        ...(readable ? { agent_id: 'agent-1' } : {}),
        events: [],
      });
    if (path.endsWith('/agents/agent-1'))
      return Response.json({ ...WIRE_AGENT, status: 'starting' });
    if (path.endsWith('/sandboxes/sandbox-1'))
      return Response.json({
        ...WIRE_SANDBOX_DETAIL,
        owner: 'owner',
        lifecycle: { pause: { allowed: false }, revive: { allowed: false } },
      });
    throw new Error(`Unexpected request ${path}`);
  });
  vi.stubGlobal('fetch', transport);
  const client = taskClient(transport);
  const onCreated = vi.fn();
  const props = { client, viewerId: 'owner', canCreateSandbox: true, onCreated };
  const first = render(<TaskHome {...props} />);
  await waitFor(() =>
    expect(screen.queryByText('Checking task creation…')).not.toBeInTheDocument(),
  );
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: '比较三个工具' } });
  const start = screen.getByRole('button', { name: 'Start task' });
  fireEvent.click(start);
  fireEvent.click(start);
  await screen.findByText('Getting its cloud computer ready');
  first.unmount();
  readable = true;
  render(<TaskHome {...props} />);
  await waitFor(() =>
    expect(onCreated).toHaveBeenCalledWith({ kind: 'agent', agentId: 'agent-1' }),
  );
  expect(requests).toEqual([
    { body: { name: 'New task', initial_message: '比较三个工具' }, key: expect.any(String) },
  ]);
});

it.each([true, false])(
  'recovers the frozen exact task without overwriting the next draft when policy is %s',
  async (enabled) => {
    const requests: { body: unknown; key: string | null }[] = [];
    let resolve!: (value: Response) => void;
    const transport = vi.fn<typeof fetch>(async (input, options) => {
      if (options?.method === 'POST') {
        requests.push({
          body: JSON.parse(String(options.body)),
          key: new Headers(options.headers).get('Idempotency-Key'),
        });
        if (requests.length === 1) throw new TypeError('Response lost');
        return new Promise<Response>((done) => {
          resolve = done;
        });
      }
      if (String(input).includes('agent-lifecycle'))
        return Response.json({
          id: 'original-operation',
          status: 'queued',
          phase: 'queued',
          events: [],
        });
      throw new Error('Unexpected read');
    });
    vi.stubGlobal('fetch', transport);
    const client = taskClient(transport);
    const props = { client, viewerId: 'owner', canCreateSandbox: true, onCreated: vi.fn() };
    const first = render(<TaskHome {...props} />);
    await waitFor(() =>
      expect(screen.queryByText('Checking task creation…')).not.toBeInTheDocument(),
    );
    fireEvent.change(screen.getByLabelText('Your task'), {
      target: { value: '  比较三个工具\nexact text  ' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
    await screen.findByText(
      "We couldn't confirm your task was received. Use Recover original task to resend the same request.",
    );
    fireEvent.change(screen.getByLabelText('Follow-up message'), {
      target: { value: 'Next independent draft' },
    });
    first.unmount();
    client.taskExperience = taskExperience('owner', enabled);
    render(<TaskHome {...props} />);
    expect(screen.getByLabelText('Follow-up message')).toHaveValue('Next independent draft');
    expect(screen.queryByRole('button', { name: 'Start task' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Recover original task' }));
    await waitFor(() => expect(requests).toHaveLength(2));
    fireEvent.change(screen.getByLabelText('Follow-up message'), {
      target: { value: 'Newer draft while recovering' },
    });
    resolve(Response.json({ id: 'original-operation', status: 'provisioning' }, { status: 202 }));
    await screen.findByText('Getting its cloud computer ready');
    expect(requests[1]).toEqual(requests[0]);
    expect(screen.getByLabelText('Follow-up message')).toHaveValue('Newer draft while recovering');
  },
);

it('shows terminal preparation failure and requires an explicit new task before changing its key', async () => {
  const keys: (string | null)[] = [];
  const transport: typeof fetch = async (_input, options) => {
    if (options?.method === 'POST') {
      keys.push(new Headers(options.headers).get('Idempotency-Key'));
      return Response.json(
        { id: `failed-operation-${keys.length}`, status: 'provisioning' },
        { status: 202 },
      );
    }
    return Response.json({
      id: `failed-operation-${keys.length}`,
      status: 'failed',
      phase: 'sandbox_ready',
      error: 'Environment unavailable',
      events: [],
    });
  };
  render(
    <TaskHome
      client={taskClient(transport)}
      viewerId="owner"
      canCreateSandbox
      onCreated={() => {}}
    />,
  );
  await waitFor(() =>
    expect(screen.queryByText('Checking task creation…')).not.toBeInTheDocument(),
  );
  fireEvent.change(screen.getByLabelText('Your task'), {
    target: { value: 'Keep the task draft' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await waitFor(() =>
    expect(screen.getByRole('alert')).toHaveTextContent('Environment unavailable'),
  );
  expect(screen.queryByText('Getting its cloud computer ready')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Start task' })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Start another task' }));
  expect(keys).toHaveLength(1);
  expect(screen.getByLabelText('Your task')).toHaveValue('Keep the task draft');
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await waitFor(() => expect(keys).toHaveLength(2));
  expect(keys[1]).not.toEqual(keys[0]);
});
