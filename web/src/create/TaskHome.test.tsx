import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { createRestWebClient } from '../api/rest';
import { WIRE_AGENT, WIRE_SANDBOX_DETAIL } from '../test/rest';
import { TaskHome } from './TaskHome';
import { AuthContextProvider, useAuth } from '../auth/context';
import type { AuthProvider } from '../auth/types';
import type { CmaWebClient } from '../api/client';
import { CreationFlow } from './CreationFlow';
import { creationClient } from '../test/creation';
import { taskExperience } from '../test/taskExperience';

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

it.each([
  { initial_message: { damaged: true } },
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
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
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

function taskClient(transport: typeof fetch, repositorySupport = true) {
  const boundary: typeof fetch = async (input, options) =>
    String(input).endsWith('/openapi.json')
      ? Response.json({
          openapi: '3.1.0',
          info: { title: 'CMA', version: '1' },
          paths: {},
          'x-cma-task-repository': repositorySupport,
          components: {
            schemas: {
              AgentLifecycleCreateRequest: {
                properties: { name: { type: 'string' }, initial_message: { type: 'string' } },
              },
            },
          },
        })
      : transport(input, options);
  vi.stubGlobal('fetch', boundary);
  return {
    ...createRestWebClient(createClient({ fetch: boundary })),
    taskExperience: taskExperience('owner'),
  };
}

it('keeps ordinary tasks available when the service does not support repository tasks', async () => {
  render(
    <TaskHome
      client={taskClient(vi.fn(), false)}
      viewerId="owner"
      canCreateSandbox
      onCreated={() => {}}
    />,
  );
  expect(
    await screen.findByText('Repository tasks are unavailable on this service.'),
  ).toBeVisible();
  expect(screen.queryByLabelText('GitHub repository')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled();
});

it('restores repository drafts and clears only their optional context before starting an empty workspace', async () => {
  const writes: unknown[] = [];
  const client = taskClient(async (_, options) => {
    if (options?.method === 'POST') writes.push(JSON.parse(String(options.body)));
    throw new Error('Lost response');
  });
  const props = { client, viewerId: 'owner', canCreateSandbox: true, onCreated: vi.fn() };
  const first = render(<TaskHome {...props} />);
  await screen.findByLabelText('GitHub repository');
  fireEvent.change(screen.getByLabelText('Your task'), {
    target: { value: 'Keep this exact draft\n ' },
  });
  fireEvent.change(screen.getByLabelText('GitHub repository'), {
    target: { value: 'https://github.com/example/project' },
  });
  fireEvent.change(screen.getByLabelText('Repository ref'), { target: { value: 'proof/fixed' } });
  first.unmount();
  render(<TaskHome {...props} />);
  expect(await screen.findByLabelText('GitHub repository')).toHaveValue(
    'https://github.com/example/project',
  );
  expect(screen.getByLabelText('Repository ref')).toHaveValue('proof/fixed');
  expect(screen.getByLabelText('Your task')).toHaveValue('Keep this exact draft\n ');
  fireEvent.click(screen.getByRole('button', { name: 'Clear repository' }));
  expect(screen.getByLabelText('Your task')).toHaveValue('Keep this exact draft\n ');
  expect(screen.getByLabelText('Repository ref')).toHaveValue('');
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await screen.findByRole('button', { name: 'Recover original task' });
  expect(writes).toEqual([{ name: 'New task', initial_message: 'Keep this exact draft\n ' }]);
});

function AuthenticatedTask({ client }: { readonly client: CmaWebClient }) {
  const { state, signOut } = useAuth();
  return state.status === 'authenticated' ? (
    <>
      <button onClick={() => void signOut()}>Sign out</button>
      <TaskHome client={client} viewerId={state.user.id} canCreateSandbox onCreated={() => {}} />
    </>
  ) : (
    <p>Signed out</p>
  );
}

function identity(owner: string): AuthProvider {
  return {
    current: async () => ({
      status: 'authenticated',
      user: {
        id: owner,
        handle: owner,
        visitor: false,
        capabilities: ['use_own_sandboxes', 'create_sandbox'],
      },
    }),
    signIn: async () => {},
    signOut: async () => {},
  };
}

it.each([
  ['blank', ' \n ', true, 'Describe a task before starting.'],
  ['UTF-8 byte limit', '你'.repeat(87382), true, 'Your task exceeds the 262144-byte limit.'],
  ['permission', 'Compare tools', false, 'You do not have permission to create a new Sandbox.'],
])(
  'does not admit a task with invalid %s and explains why',
  async (_name, text, permitted, error) => {
    const transport = vi.fn<typeof fetch>();
    vi.stubGlobal('fetch', transport);
    const client = taskClient(transport);
    render(
      <TaskHome
        client={client}
        viewerId="owner"
        canCreateSandbox={Boolean(permitted)}
        onCreated={() => {}}
      />,
    );
    await waitFor(() =>
      expect(screen.queryByText('Checking task creation…')).not.toBeInTheDocument(),
    );
    fireEvent.change(screen.getByLabelText('Your task'), { target: { value: text } });
    fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
    expect(screen.queryAllByRole('alert').map((element) => element.textContent)).toContain(error);
    expect(transport).not.toHaveBeenCalled();
  },
);

it('clears drafts and retained requests on sign-out and account change before reauthentication restores Home', async () => {
  const transport = vi.fn<typeof fetch>(async () => {
    throw new TypeError('Response lost');
  });
  vi.stubGlobal('fetch', transport);
  const client = taskClient(transport);
  const first = render(
    <AuthContextProvider provider={identity('owner')}>
      <AuthenticatedTask client={client} />
    </AuthContextProvider>,
  );
  fireEvent.change(await screen.findByLabelText('Your task'), {
    target: { value: 'Private owner task' },
  });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await screen.findByText('Response lost');
  fireEvent.click(screen.getByRole('button', { name: 'Sign out' }));
  await screen.findByText('Signed out');
  first.unmount();
  const second = render(
    <AuthContextProvider provider={identity('owner')}>
      <AuthenticatedTask client={client} />
    </AuthContextProvider>,
  );
  expect(await screen.findByLabelText('Your task')).toHaveValue('');
  expect(screen.queryByText('Private owner task')).not.toBeInTheDocument();
  fireEvent.change(screen.getByLabelText('Your task'), {
    target: { value: 'Another private draft' },
  });
  second.rerender(
    <AuthContextProvider provider={identity('other')}>
      <AuthenticatedTask client={client} />
    </AuthContextProvider>,
  );
  await waitFor(() => expect(screen.getByLabelText('Your task')).toHaveValue(''));
  second.rerender(
    <AuthContextProvider provider={identity('owner')}>
      <AuthenticatedTask client={client} />
    </AuthContextProvider>,
  );
  await waitFor(() => expect(screen.getByLabelText('Your task')).toHaveValue(''));
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
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: '比较三个工具' } });
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await screen.findByText('Task accepted. Preparing your Agent.');
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
    await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
    fireEvent.change(screen.getByLabelText('Your task'), {
      target: { value: '  比较三个工具\nexact text  ' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
    await screen.findByText('Response lost');
    fireEvent.change(screen.getByLabelText('Your task'), {
      target: { value: 'Next independent draft' },
    });
    first.unmount();
    client.taskExperience = taskExperience('owner', enabled);
    render(<TaskHome {...props} />);
    expect(screen.getByLabelText('Your task')).toHaveValue('Next independent draft');
    if (!enabled) expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Recover original task' }));
    await waitFor(() => expect(requests).toHaveLength(2));
    fireEvent.change(screen.getByLabelText('Your task'), {
      target: { value: 'Newer draft while recovering' },
    });
    resolve(Response.json({ id: 'original-operation', status: 'provisioning' }, { status: 202 }));
    await screen.findByText('Task accepted. Preparing your Agent.');
    expect(requests[1]).toEqual(requests[0]);
    expect(screen.getByLabelText('Your task')).toHaveValue('Newer draft while recovering');
  },
);

it('explains that an older service cannot start tasks without silently creating an empty Agent', async () => {
  const writes: string[] = [];
  const transport = vi.fn<typeof fetch>(async (input, options) => {
    if (options?.method === 'POST') writes.push(String(input));
    return Response.json({
      openapi: '3.1.0',
      info: { title: 'CMA', version: '1' },
      paths: {},
      components: {
        schemas: { AgentLifecycleCreateRequest: { properties: { name: { type: 'string' } } } },
      },
    });
  });
  vi.stubGlobal('fetch', transport);
  const client = createRestWebClient(createClient({ fetch: transport }));
  render(<TaskHome client={client} viewerId="owner" canCreateSandbox onCreated={() => {}} />);
  await screen.findByText('Task creation is unavailable on this service.');
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  expect(writes).toEqual([]);
});

it('opens ordinary contextual creation as editable task templates without admitting work', async () => {
  const transport = vi.fn<typeof fetch>();
  const client = taskClient(transport);
  render(
    <CreationFlow
      request={{ kind: 'new_agent', workspaceId: 'workspace-owned' }}
      viewerId="owner"
      client={client}
      catalog={creationClient()}
      canCreateSandbox={false}
      onClose={() => {}}
      onCreated={() => {}}
      onChanged={() => {}}
    />,
  );
  expect(screen.getByLabelText('Your task')).toHaveValue('');
  expect(screen.getByText('Creating in Workspace workspace-owned')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Research' }));
  expect(screen.getByLabelText('Your task')).toHaveValue(
    'Compare three tools and summarize their strengths, limitations, and sources.',
  );
  expect(screen.getByLabelText('Your task')).toHaveFocus();
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
  expect(transport).not.toHaveBeenCalled();
});

it('verifies the Agent even when the first status read already reports preparation succeeded', async () => {
  const transport = vi.fn<typeof fetch>(async (input, options) => {
    if (options?.method === 'POST')
      return Response.json(
        { id: 'completed-preparation', status: 'provisioning' },
        { status: 202 },
      );
    const path = String(input);
    if (path.includes('/agent-lifecycle/'))
      return Response.json({
        id: 'completed-preparation',
        status: 'succeeded',
        phase: 'agent_startup',
        agent_id: 'agent-1',
        events: [],
      });
    if (path.endsWith('/agents/agent-1')) return Response.json(WIRE_AGENT);
    if (path.endsWith('/sandboxes/sandbox-1'))
      return Response.json({
        ...WIRE_SANDBOX_DETAIL,
        owner: 'owner',
        lifecycle: { pause: { allowed: false }, revive: { allowed: false } },
      });
    throw new Error('Unexpected read');
  });
  const onCreated = vi.fn();
  render(
    <TaskHome
      client={taskClient(transport)}
      viewerId="owner"
      canCreateSandbox
      onCreated={onCreated}
    />,
  );
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'Compare tools' } });
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await waitFor(() =>
    expect(onCreated).toHaveBeenCalledExactlyOnceWith({ kind: 'agent', agentId: 'agent-1' }),
  );
});

it('resumes a desktop repository task only on explicit connection action with its frozen options, body and key after cancel and refresh', async () => {
  const writes: { body: unknown; key: string | null }[] = [];
  let checks = 0;
  let authorized = false;
  const transport: typeof fetch = async (input, options) => {
    const path = String(input);
    if (path === '/auth/connections/check') {
      checks += 1;
      return Response.json({
        owner: 'owner',
        ready: authorized,
        checked_at: '2026-11-03T00:00:00Z',
        services: [
          {
            service: 'api-github',
            display_name: 'GitHub',
            kind: 'nyxid',
            state: authorized ? 'satisfied' : 'missing',
            connection_mode: 'hosted',
          },
        ],
      });
    }
    if (options?.method === 'POST') {
      writes.push({
        body: JSON.parse(String(options.body)),
        key: new Headers(options.headers).get('Idempotency-Key'),
      });
      return Response.json({ id: 'connection-operation', status: 'provisioning' }, { status: 202 });
    }
    if (path.includes('/agent-lifecycle/'))
      return Response.json({
        id: 'connection-operation',
        status: 'waiting_for_connection',
        phase: 'key_scope_ready',
        connection: {
          services: ['api-github'],
          target: { type: 'sandbox_profile', profile: { id: 'profile-owned', revision: 1 } },
        },
        events: [],
      });
    throw new Error('Unexpected read');
  };
  const client = {
    ...taskClient(transport),
    readDesktopCapability: async () => ({
      state: 'available' as const,
      reason: 'Verified desktop runtime',
      runtime_contract: 3,
      min_memory_mib: 2048,
      min_cpu_millis: 1000,
    }),
  };
  const props = { client, viewerId: 'owner', canCreateSandbox: true, onCreated: vi.fn() };
  const first = render(<TaskHome {...props} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
  const desktop = screen.getByRole('checkbox', { name: 'Enable desktop' });
  await waitFor(() => expect(desktop).toBeEnabled());
  fireEvent.click(desktop);
  fireEvent.change(screen.getByLabelText('Your task'), {
    target: { value: '  Original task\nwith whitespace  ' },
  });
  fireEvent.change(screen.getByLabelText('GitHub repository'), {
    target: { value: 'https://github.com/example/project' },
  });
  fireEvent.change(screen.getByLabelText('Repository ref'), {
    target: { value: 'proof/fixed' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await screen.findByRole('button', { name: 'Continue original task' });
  expect(checks).toBe(0);
  fireEvent.click(screen.getByRole('button', { name: 'Continue original task' }));
  await screen.findByRole('dialog', { name: 'Connect your accounts' });
  fireEvent.keyDown(document, { key: 'Escape' });
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(desktop).toBeChecked();
  expect(desktop).toBeDisabled();
  expect(writes).toHaveLength(1);
  const checksBeforeRefresh = checks;
  first.unmount();
  authorized = true;
  render(<TaskHome {...props} />);
  await screen.findByRole('button', { name: 'Continue original task' });
  expect(screen.getByRole('checkbox', { name: 'Enable desktop' })).toBeChecked();
  expect(screen.getByRole('checkbox', { name: 'Enable desktop' })).toBeDisabled();
  expect(writes).toHaveLength(1);
  expect(checks).toBe(checksBeforeRefresh);
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'Next task draft' } });
  fireEvent.click(screen.getByRole('button', { name: 'Continue original task' }));
  await waitFor(() => expect(writes).toHaveLength(2));
  expect(writes[1]).toEqual(writes[0]);
  expect(writes[0]?.body).toMatchObject({
    initial_message: '  Original task\nwith whitespace  ',
    repository: { url: 'https://github.com/example/project', ref: 'proof/fixed' },
    desktop: true,
  });
  expect(checks).toBe(checksBeforeRefresh + 1);
  expect(screen.getByLabelText('Your task')).toHaveValue('Next task draft');
});

it.each(['client', 'account', 'context', 'unmount'] as const)(
  'ignores an old admission callback after a %s change',
  async (change) => {
    let resolve!: (response: Response) => void;
    let writes = 0;
    const transport: typeof fetch = async (_input, options) => {
      if (options?.method === 'POST') {
        writes += 1;
        return new Promise<Response>((done) => {
          resolve = done;
        });
      }
      return Response.json({ id: 'old-operation', status: 'queued', phase: 'queued', events: [] });
    };
    const props = {
      client: taskClient(transport),
      viewerId: 'owner',
      canCreateSandbox: true,
      onCreated: vi.fn(),
    };
    const view = render(<TaskHome {...props} />);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
    fireEvent.change(screen.getByLabelText('Your task'), {
      target: { value: 'Original scope task' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
    await waitFor(() => expect(writes).toBe(1));
    if (change === 'unmount') view.unmount();
    else
      view.rerender(
        <TaskHome
          {...props}
          {...(change === 'client'
            ? { client: taskClient(transport) }
            : change === 'account'
              ? { viewerId: 'other' }
              : { workspaceId: 'another-workspace' })}
        />,
      );
    await act(async () =>
      resolve(Response.json({ id: 'old-operation', status: 'provisioning' }, { status: 202 })),
    );
    expect(screen.queryByText('Task accepted. Preparing your Agent.')).not.toBeInTheDocument();
    expect(props.onCreated).not.toHaveBeenCalled();
    expect(writes).toBe(1);
  },
);

it('keeps the receipt without opening an Agent whose parent ownership is not proven', async () => {
  let parentReads = 0;
  const transport: typeof fetch = async (input, options) => {
    if (options?.method === 'POST')
      return Response.json({ id: 'owned-operation', status: 'provisioning' }, { status: 202 });
    const path = String(input);
    if (path.includes('/agent-lifecycle/'))
      return Response.json({
        id: 'owned-operation',
        status: 'succeeded',
        phase: 'agent_startup',
        agent_id: 'agent-1',
        events: [],
      });
    if (path.endsWith('/agents/agent-1')) return Response.json(WIRE_AGENT);
    parentReads += 1;
    return Response.json({
      ...WIRE_SANDBOX_DETAIL,
      owner: undefined,
      lifecycle: { pause: { allowed: false }, revive: { allowed: false } },
    });
  };
  const onCreated = vi.fn();
  render(
    <TaskHome
      client={taskClient(transport)}
      viewerId="owner"
      canCreateSandbox
      onCreated={onCreated}
    />,
  );
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
  fireEvent.change(screen.getByLabelText('Your task'), {
    target: { value: 'Verify exact Agent identity' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await waitFor(() => expect(parentReads).toBeGreaterThan(0));
  expect(onCreated).not.toHaveBeenCalled();
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
});

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
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
  fireEvent.change(screen.getByLabelText('Your task'), {
    target: { value: 'Keep the task draft' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await screen.findByText('Environment unavailable');
  expect(screen.queryByText('Task accepted. Preparing your Agent.')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Start another task' }));
  expect(keys).toHaveLength(1);
  expect(screen.getByLabelText('Your task')).toHaveValue('Keep the task draft');
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await waitFor(() => expect(keys).toHaveLength(2));
  expect(keys[1]).not.toEqual(keys[0]);
});
