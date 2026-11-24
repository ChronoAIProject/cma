import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AuthContextProvider } from '../auth/context';
import { WIRE_AGENT, WIRE_SANDBOX_DETAIL } from '../test/rest';
import { TaskHome } from './TaskHome';
import { AuthenticatedTask, computerOption, identity, taskClient } from './taskHomeTestSupport';

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

it('keeps a managed repository wait on the same request until an explicit recheck', async () => {
  const writes: { key: string | null; body: unknown }[] = [];
  const manageUrl = 'https://nyx.example/keys';
  const weakConnection = {
    id: 'profile-only',
    label: 'GitHub profile access',
    granted_scopes: ['read:user'],
    manage_url: `${manageUrl}/profile-only`,
  };
  const repositoryConnection = {
    id: 'repo-access',
    label: 'GitHub repository access',
    granted_scopes: ['repo'],
    manage_url: `${manageUrl}/repo-access`,
  };
  const readInventory = vi.fn(async () =>
    Response.json({
      owner: 'owner',
      github: {
        manage_url: manageUrl,
        connections:
          writes.length === 1 ? [weakConnection, repositoryConnection] : [repositoryConnection],
      },
    }),
  );
  const transport: typeof fetch = async (input, options) => {
    if (String(input) === '/auth/connections/repository-access') {
      return readInventory();
    }
    if (options?.method === 'POST') {
      writes.push({
        key: new Headers(options.headers).get('Idempotency-Key'),
        body: JSON.parse(String(options.body)),
      });
      return Response.json({ id: 'repository-operation', status: 'provisioning' }, { status: 202 });
    }
    if (String(input).includes('/agent-lifecycle/')) {
      return Response.json({
        id: 'repository-operation',
        status: 'waiting_for_connection',
        phase: 'admission',
        connection: {
          services: ['api-github'],
          target: { type: 'sandbox_profile', profile: { id: 'profile-owned', revision: 1 } },
          repository_access: 'verification_unavailable',
        },
        events: [],
      });
    }
    throw new Error('Unexpected read');
  };
  render(
    <TaskHome
      client={taskClient(transport)}
      viewerId="owner"
      canCreateSandbox
      onCreated={vi.fn()}
    />,
  );
  await waitFor(() =>
    expect(screen.queryByText('Checking task creation…')).not.toBeInTheDocument(),
  );
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'Inspect the source' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add git repo' }));
  fireEvent.change(screen.getByLabelText('GitHub repository'), {
    target: { value: 'https://github.com/example/project' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Add repo' }));
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  const weakLink = await screen.findByRole('link', {
    name: 'GitHub profile access (profile-only)',
  });
  expect(
    screen.getByRole('link', { name: 'GitHub repository access (repo-access)' }),
  ).toBeVisible();
  expect(writes).toHaveLength(1);
  expect(readInventory).toHaveBeenCalledOnce();
  expect(screen.queryByRole('button', { name: 'Continue original task' })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Recheck repository access' }));
  await waitFor(() => expect(writes).toHaveLength(2));
  expect(writes[1]).toEqual(writes[0]);
  await waitFor(() => expect(weakLink).not.toBeInTheDocument());
  expect(
    screen.getByRole('link', { name: 'GitHub repository access (repo-access)' }),
  ).toBeVisible();
  expect(readInventory).toHaveBeenCalledTimes(2);
});

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
  await waitFor(() =>
    expect(screen.queryByText('Checking task creation…')).not.toBeInTheDocument(),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await screen.findByText(
    "We couldn't confirm your task was received. Use Recover original task to resend the same request.",
  );
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
  await waitFor(() =>
    expect(screen.queryByText('Checking task creation…')).not.toBeInTheDocument(),
  );
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'Compare tools' } });
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await waitFor(() =>
    expect(onCreated).toHaveBeenCalledExactlyOnceWith({ kind: 'agent', agentId: 'agent-1' }),
  );
});

it('resumes shared desktop and repository only on explicit connection action with frozen options, body and key after cancel and refresh', async () => {
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
  const client = taskClient(transport);
  const props = { client, viewerId: 'owner', canCreateSandbox: true, onCreated: vi.fn() };
  const first = render(<TaskHome {...props} />);
  await waitFor(() =>
    expect(screen.queryByText('Checking task creation…')).not.toBeInTheDocument(),
  );
  const machine = computerOption('Shared desktop');
  fireEvent.click(machine);
  fireEvent.change(screen.getByLabelText('Your task'), {
    target: { value: '  Original task\nwith whitespace  ' },
  });
  fireEvent.keyDown(document, { key: 'Escape' });
  fireEvent.click(screen.getByRole('button', { name: 'Add git repo' }));
  fireEvent.change(screen.getByLabelText('GitHub repository'), {
    target: { value: 'https://github.com/example/project' },
  });
  fireEvent.change(screen.getByLabelText('Repository ref'), {
    target: { value: 'proof/fixed' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Add repo' }));
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await screen.findByRole('button', { name: 'Continue original task' });
  expect(checks).toBe(0);
  fireEvent.click(screen.getByRole('button', { name: 'Continue original task' }));
  await screen.findByRole('dialog', { name: 'Connect your accounts' });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Close' })).toHaveFocus());
  fireEvent.keyDown(document, { key: 'Escape' });
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(screen.queryByRole('button', { name: /^Cloud computer/ })).not.toBeInTheDocument();
  expect(screen.getByText('Shared desktop')).toBeInTheDocument();
  expect(writes).toHaveLength(1);
  const checksBeforeRefresh = checks;
  first.unmount();
  authorized = true;
  render(<TaskHome {...props} />);
  await screen.findByRole('button', { name: 'Continue original task' });
  expect(screen.queryByRole('button', { name: /^Cloud computer/ })).not.toBeInTheDocument();
  expect(screen.getByText('Shared desktop')).toBeInTheDocument();
  expect(writes).toHaveLength(1);
  expect(checks).toBe(checksBeforeRefresh);
  fireEvent.change(screen.getByLabelText('Follow-up draft'), {
    target: { value: 'Next task draft' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Continue original task' }));
  await waitFor(() => expect(writes).toHaveLength(2));
  expect(writes[1]).toEqual(writes[0]);
  expect(writes[0]?.body).toMatchObject({
    initial_message: '  Original task\nwith whitespace  ',
    repository: { url: 'https://github.com/example/project', ref: 'proof/fixed' },
    machine_included: true,
  });
  expect(checks).toBe(checksBeforeRefresh + 1);
  expect(screen.getByLabelText('Follow-up draft')).toHaveValue('Next task draft');
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
    await waitFor(() =>
      expect(screen.queryByText('Checking task creation…')).not.toBeInTheDocument(),
    );
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
    expect(
      screen.queryByText('Preparing a cloud computer for your Agent…'),
    ).not.toBeInTheDocument();
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
  await waitFor(() =>
    expect(screen.queryByText('Checking task creation…')).not.toBeInTheDocument(),
  );
  fireEvent.change(screen.getByLabelText('Your task'), {
    target: { value: 'Verify exact Agent identity' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await waitFor(() => expect(parentReads).toBeGreaterThan(0));
  expect(onCreated).not.toHaveBeenCalled();
  expect(screen.queryByRole('button', { name: 'Start task' })).not.toBeInTheDocument();
});
