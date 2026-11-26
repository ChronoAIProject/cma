import { AVAILABLE_CAPACITY } from '../test/capacity';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { createRestWebClient } from '../api/rest';
import { taskExperience } from '../test/taskExperience';
import { TaskHome } from './TaskHome';

afterEach(() => {
  sessionStorage.clear();
  vi.unstubAllGlobals();
});

function fixture(refusal?: { code: string; detail: string }) {
  const writes: { body: unknown; key: string | null }[] = [];
  const transport: typeof fetch = async (input, options) => {
    if (options?.method === 'POST') {
      writes.push({
        body: JSON.parse(String(options.body)),
        key: new Headers(options.headers).get('Idempotency-Key'),
      });
      if (refusal) return Response.json(refusal, { status: 422 });
      throw new TypeError('Creation receipt lost');
    }
    if (String(input).endsWith('/openapi.json'))
      return Response.json({
        openapi: '3.1.0',
        info: { title: 'CMA', version: '1' },
        paths: {},
        components: {
          schemas: {
            AgentLifecycleCreateRequest: {
              properties: {
                initial_message: { type: 'string' },
                machine_included: { type: 'boolean' },
              },
            },
          },
        },
      });
    throw new Error(`Unexpected read: ${String(input)}`);
  };
  vi.stubGlobal('fetch', transport);
  const client = {
    ...createRestWebClient(createClient({ fetch: transport })),
    readSandboxCapacity: async () => AVAILABLE_CAPACITY,
    taskExperience: taskExperience('owner'),
  };
  return { client, writes };
}

it.each([false, true])(
  'freezes shared desktop choice %s through draft refresh and receipt recovery',
  async (machine) => {
    const { client, writes } = fixture();
    const props = { client, viewerId: 'owner', canCreateSandbox: true, onCreated: vi.fn() };
    const task = '  Original task\nkeep exact text  ';
    const first = render(<TaskHome {...props} />);
    const machineOption = computerOption('Shared desktop');
    expect(
      screen.queryByRole('menuitemcheckbox', { name: 'Agent browser' }),
    ).not.toBeInTheDocument();
    expect(screen.getAllByRole('menuitemcheckbox')).toHaveLength(1);
    expect(machineOption).not.toBeChecked();
    if (machine) fireEvent.click(machineOption);
    expect(screen.getByRole('button', { name: /^Cloud computer/ })).toHaveTextContent(
      machine ? 'Cloud computer · Shared desktop' : 'Cloud computer',
    );
    fireEvent.change(screen.getByLabelText('Your task'), { target: { value: task } });
    expect(writes).toHaveLength(0);
    first.unmount();
    const refreshed = render(<TaskHome {...props} />);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
    expect(computerOption('Shared desktop')).toHaveAttribute('aria-checked', String(machine));
    expect(screen.getByLabelText('Your task')).toHaveValue(task);
    fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
    await screen.findByText(
      "We couldn't confirm your task was received. Use Recover original task to resend the same request.",
    );
    refreshed.unmount();
    render(<TaskHome {...props} />);
    expect(screen.queryByRole('button', { name: /^Cloud computer/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByText('Setup details'));
    expect(screen.getByRole('definition')).toHaveTextContent(
      machine ? 'Shared desktop' : 'Cloud computer',
    );
    expect(
      screen.queryByRole('menuitemcheckbox', { name: 'Agent browser' }),
    ).not.toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('Follow-up draft'), {
      target: { value: 'Next task draft' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Recover original task' }));
    await waitFor(() => expect(writes).toHaveLength(2));
    expect(writes[1]).toEqual(writes[0]);
    expect(writes[0]).toEqual({
      body: {
        name: 'New task',
        initial_message: task,
        ...(machine ? { machine_included: true } : {}),
      },
      key: expect.any(String),
    });
  },
);

it.each([
  { workspaceId: 'workspace-owned', canCreateSandbox: true },
  { sandboxId: 'sandbox-owned', canCreateSandbox: true },
  { canCreateSandbox: false },
])(
  'disables adding a shared desktop outside authorized new Sandbox creation: %j',
  async (scope) => {
    const { client, writes } = fixture();
    render(<TaskHome client={client} viewerId="owner" {...scope} onCreated={() => {}} />);
    await waitFor(() =>
      expect(screen.queryByText('Checking task creation…')).not.toBeInTheDocument(),
    );
    computerOption('Shared desktop');
    expect(
      screen.queryByRole('menuitemcheckbox', { name: 'Agent browser' }),
    ).not.toBeInTheDocument();
    expect(computerOption('Shared desktop')).toHaveAttribute('aria-disabled', 'true');
    expect(screen.getByText(/an existing Sandbox keeps its original setup/)).toBeVisible();
    expect(writes).toHaveLength(0);
  },
);

it('lets an unavailable retained shared desktop draft clear its choice without losing the task', async () => {
  sessionStorage.setItem(
    `cma:task-draft:${JSON.stringify([location.origin, 'owner', 'workspace-owned', null])}`,
    JSON.stringify({ version: 1, text: 'Keep my task', machineIncluded: true }),
  );
  const { client, writes } = fixture();
  render(
    <TaskHome
      client={client}
      viewerId="owner"
      workspaceId="workspace-owned"
      canCreateSandbox
      onCreated={() => {}}
    />,
  );
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  expect(screen.getByRole('alert')).toHaveTextContent('A shared desktop requires a new Sandbox');
  expect(writes).toHaveLength(0);
  const option = computerOption('Shared desktop');
  expect(option).toBeChecked();
  expect(option).toHaveAttribute('aria-disabled', 'false');
  fireEvent.click(option);
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await screen.findByText(
    "We couldn't confirm your task was received. Use Recover original task to resend the same request.",
  );
  expect(writes[0]?.body).toEqual({
    name: 'New task',
    initial_message: 'Keep my task',
    workspace_id: 'workspace-owned',
  });
});

it('retains shared desktop on a typed refusal without silently selecting another image', async () => {
  const refusal = {
    code: 'desktop_unsupported',
    detail: 'This image is not verified for desktop creation.',
  };
  const { client, writes } = fixture(refusal);
  const props = { client, viewerId: 'owner', canCreateSandbox: true, onCreated: vi.fn() };
  const first = render(<TaskHome {...props} />);
  await waitFor(() =>
    expect(screen.queryByText('Checking task creation…')).not.toBeInTheDocument(),
  );
  fireEvent.click(computerOption('Shared desktop'));
  fireEvent.change(screen.getByLabelText('Your task'), {
    target: { value: 'Keep the shared desktop' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await screen.findByText(refusal.detail);
  expect(props.onCreated).not.toHaveBeenCalled();
  first.unmount();
  render(<TaskHome {...props} />);
  expect(computerOption('Shared desktop')).toBeChecked();
  expect(writes).toHaveLength(1);
  expect(screen.queryByRole('button', { name: 'Recover original task' })).not.toBeInTheDocument();
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await screen.findByText(refusal.detail);
  expect(writes).toHaveLength(2);
  expect(writes[1]?.body).toEqual(writes[0]?.body);
  expect(writes[1]?.key).not.toEqual(writes[0]?.key);
  expect(writes[0]?.body).toEqual({
    name: 'New task',
    initial_message: 'Keep the shared desktop',
    machine_included: true,
  });
});

function computerOption(name: string) {
  if (!screen.queryByRole('menu', { name: 'Run this task on' }))
    fireEvent.click(screen.getByRole('button', { name: /^Cloud computer/ }));
  return screen.getByRole('menuitemcheckbox', { name });
}
