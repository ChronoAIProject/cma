import { selectTaskTemplate } from '../test/taskTemplateSelection';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { createRestWebClient } from '../api/rest';
import { TaskHome } from './TaskHome';
import { CreationFlow } from './CreationFlow';
import { creationClient } from '../test/creation';
import { taskClient } from './taskHomeTestSupport';

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

it('keeps ordinary tasks available when the service does not support repository tasks', async () => {
  render(
    <TaskHome
      client={taskClient(vi.fn(), false)}
      viewerId="owner"
      canCreateSandbox
      onCreated={() => {}}
    />,
  );
  expect(screen.getByRole('button', { name: 'Add git repo' })).toBeDisabled();
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'An ordinary task' } });
  expect(
    await screen.findByText('Repository tasks are unavailable on this service.'),
  ).toBeVisible();
  expect(screen.queryByLabelText('GitHub repository')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled();
});

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
    if (_name === 'blank') {
      expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
      fireEvent.submit(screen.getByLabelText('Your task').closest('form')!);
    } else fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
    expect(screen.queryAllByRole('alert').map((element) => element.textContent)).toContain(error);
    expect(transport).not.toHaveBeenCalled();
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
  selectTaskTemplate('Research a topic');
  expect(screen.getByLabelText('Topic')).toHaveValue('');
  await waitFor(() => expect(screen.getByLabelText('Topic')).toHaveFocus());
  await waitFor(() =>
    expect(screen.queryByText('Checking task creation…')).not.toBeInTheDocument(),
  );
  expect(transport).not.toHaveBeenCalled();
});
