import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { createRestWebClient } from '../api/rest';
import { taskExperience } from '../test/taskExperience';
import { TaskHome } from './TaskHome';

afterEach(() => {
  sessionStorage.clear();
  vi.unstubAllGlobals();
});

function fixture(browserAvailable = true, refusal?: { code: string; detail: string }) {
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
                desktop: { type: 'boolean' },
                machine_included: { type: 'boolean' },
              },
            },
          },
        },
      });
    return Response.json({
      state: browserAvailable ? 'available' : 'unsupported',
      reason: 'Agent browser capability',
      runtime_contract: 3,
      min_memory_mib: 2048,
      min_cpu_millis: 1000,
    });
  };
  vi.stubGlobal('fetch', transport);
  const client = {
    ...createRestWebClient(createClient({ fetch: transport })),
    taskExperience: taskExperience('owner'),
  };
  return { client, writes };
}

it.each([
  { machine: false, browser: false },
  { machine: true, browser: false },
  { machine: false, browser: true },
  { machine: true, browser: true },
])(
  'freezes independent desktop choices through draft refresh and receipt recovery: %j',
  async ({ machine, browser }) => {
    const { client, writes } = fixture();
    const props = { client, viewerId: 'owner', canCreateSandbox: true, onCreated: vi.fn() };
    const task = '  Original task\nkeep exact text  ';
    const first = render(<TaskHome {...props} />);
    const machineOption = computerOption('Shared desktop');
    const browserOption = computerOption('Agent browser');
    await waitFor(() => expect(browserOption).toHaveAttribute('aria-disabled', 'false'));
    expect(machineOption).not.toBeChecked();
    expect(browserOption).not.toBeChecked();
    if (machine) fireEvent.click(machineOption);
    if (browser) fireEvent.click(browserOption);
    fireEvent.change(screen.getByLabelText('Your task'), { target: { value: task } });
    expect(writes).toHaveLength(0);
    first.unmount();
    const refreshed = render(<TaskHome {...props} />);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
    expect(computerOption('Shared desktop')).toHaveAttribute('aria-checked', String(machine));
    expect(computerOption('Agent browser')).toHaveAttribute('aria-checked', String(browser));
    expect(screen.getByLabelText('Your task')).toHaveValue(task);
    fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
    await screen.findByText(
      "We couldn't confirm your task was received. Use Recover original task to resend the same request.",
    );
    refreshed.unmount();
    render(<TaskHome {...props} />);
    expect(computerOption('Shared desktop')).toHaveAttribute('aria-disabled', 'true');
    expect(computerOption('Agent browser')).toHaveAttribute('aria-disabled', 'true');
    fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'Next task draft' } });
    fireEvent.click(screen.getByRole('button', { name: 'Recover original task' }));
    await waitFor(() => expect(writes).toHaveLength(2));
    expect(writes[1]).toEqual(writes[0]);
    expect(writes[0]).toEqual({
      body: {
        name: 'New task',
        initial_message: task,
        ...(machine ? { machine_included: true } : {}),
        ...(browser ? { desktop: true } : {}),
      },
      key: expect.any(String),
    });
  },
);

it('keeps shared desktop creation independent when Agent browser capability is absent', async () => {
  const { client, writes } = fixture(false);
  render(<TaskHome client={client} viewerId="owner" canCreateSandbox onCreated={() => {}} />);
  computerOption('Agent browser');
  await screen.findByText('Agent browser unavailable: Agent browser capability');
  expect(computerOption('Agent browser')).toHaveAttribute('aria-disabled', 'true');
  fireEvent.click(computerOption('Shared desktop'));
  fireEvent.change(screen.getByLabelText('Your task'), {
    target: { value: 'Use the shared desktop' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await screen.findByText(
    "We couldn't confirm your task was received. Use Recover original task to resend the same request.",
  );
  expect(writes[0]?.body).toEqual({
    name: 'New task',
    initial_message: 'Use the shared desktop',
    machine_included: true,
  });
});

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

it('retains both options on a typed combination refusal without silently selecting another image', async () => {
  const refusal = {
    code: 'desktop_unsupported',
    detail: 'This image is not verified for desktop creation.',
  };
  const { client, writes } = fixture(true, refusal);
  const props = { client, viewerId: 'owner', canCreateSandbox: true, onCreated: vi.fn() };
  const first = render(<TaskHome {...props} />);
  const browser = computerOption('Agent browser');
  await waitFor(() => expect(browser).toHaveAttribute('aria-disabled', 'false'));
  fireEvent.click(browser);
  fireEvent.click(computerOption('Shared desktop'));
  fireEvent.change(screen.getByLabelText('Your task'), {
    target: { value: 'Keep both desktop choices' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await screen.findByText(refusal.detail);
  expect(props.onCreated).not.toHaveBeenCalled();
  first.unmount();
  render(<TaskHome {...props} />);
  for (const label of ['Shared desktop', 'Agent browser']) {
    expect(computerOption(label)).toBeChecked();
  }
  expect(writes).toHaveLength(1);
  expect(screen.queryByRole('button', { name: 'Recover original task' })).not.toBeInTheDocument();
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
  await waitFor(() =>
    expect(computerOption('Agent browser')).toHaveAttribute('aria-disabled', 'false'),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await screen.findByText(refusal.detail);
  expect(writes).toHaveLength(2);
  expect(writes[1]?.body).toEqual(writes[0]?.body);
  expect(writes[1]?.key).not.toEqual(writes[0]?.key);
  expect(writes[0]?.body).toEqual({
    name: 'New task',
    initial_message: 'Keep both desktop choices',
    machine_included: true,
    desktop: true,
  });
});

function computerOption(name: string) {
  if (!screen.queryByRole('menu', { name: 'Run this task on' }))
    fireEvent.click(screen.getByRole('button', { name: /^Cloud computer/ }));
  return screen.getByRole('menuitemcheckbox', { name });
}
