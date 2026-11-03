import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { connectionClient } from '../connections/client';
import { agentClient } from '../test/agent';
import { AgentSurface } from './AgentSurface';
import { useAgentController } from './controller';
import { accessRequests } from './accessRequests';
import { binding, harness } from './transport/controller.fixture';

afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});

it('preserves the same draft and uploaded attachment through settings disclosure and sends them unchanged', async () => {
  const user = userEvent.setup();
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'owner',
    ready: true,
    checked_at: '2026-11-03T10:00:00Z',
    services: [],
  });
  vi.spyOn(accessRequests, 'list').mockResolvedValue([]);
  const runAgent = vi.fn().mockResolvedValue(undefined);
  const upload = vi.fn().mockResolvedValue({
    id: 'att_notes',
    agentId: 'agt_1',
    name: 'notes.txt',
    path: '/home/attachments/agt_1/att_notes/notes.txt',
    size: 5,
    mediaType: 'text/plain',
    sha256: 'a'.repeat(64),
  });
  const client = agentClient({
    readAgentSettings: async () => ({
      model: 'gpt-6',
      reasoning_effort: 'high',
      permission_mode: 'autonomous',
    }),
    readAgentModels: async () => [{ id: 'gpt-6', label: 'GPT-6', available: true }],
    updateAgentSettings: vi.fn(),
    uploadAgentAttachment: upload,
    runAgent,
  });
  function Agent() {
    const controller = useAgentController(client, 'agt_1', { viewerId: 'owner' });
    return (
      <CmaClientProvider client={client}>
        <AgentSurface
          agentId="agt_1"
          viewerId="owner"
          lifecycleClient={client}
          controller={controller}
          detailsOpen={false}
          onDetailsOpenChange={() => undefined}
          onNavigate={() => undefined}
        />
      </CmaClientProvider>
    );
  }
  render(<Agent />);
  const field = await screen.findByRole('textbox', { name: 'Message' });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Attach files' })).toBeEnabled());
  fireEvent.change(field, { target: { value: 'Keep this draft\nand its attachment.' } });
  fireEvent.drop(field, {
    dataTransfer: { files: [new File(['notes'], 'notes.txt', { type: 'text/plain' })] },
  });
  await screen.findByText('Ready');
  const summary = await screen.findByRole('button', { name: /^Agent settings:/ });
  expect(summary).toHaveTextContent('GPT-6 · High · Autonomous');
  expect(summary).toHaveAttribute('aria-expanded', 'false');
  expect(screen.queryByRole('button', { name: 'Permission mode' })).not.toBeInTheDocument();
  await user.click(summary);
  expect(screen.getByRole('region', { name: 'Agent settings' })).toBeVisible();
  expect(screen.getByRole('button', { name: 'Permission mode' })).toBeEnabled();
  await user.click(summary);
  expect(summary).toHaveAttribute('aria-expanded', 'false');
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(field);
  expect(field).toHaveValue('Keep this draft\nand its attachment.');
  expect(screen.getByText('notes.txt')).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Send message' }));
  await waitFor(() => expect(runAgent).toHaveBeenCalledOnce());
  expect(runAgent.mock.calls[0]![0].messages).toEqual([
    {
      role: 'user',
      content:
        'Keep this draft\nand its attachment.\n\nAttached files are available in the sandbox. Read the relevant files at these paths; images can be opened with your image tool:\n' +
        '[\n  {\n    "name": "notes.txt",\n    "path": "/home/attachments/agt_1/att_notes/notes.txt",\n    "media_type": "text/plain",\n    "size_bytes": 5\n  }\n]',
    },
  ]);
  expect(upload).toHaveBeenCalledOnce();
  expect(client.updateAgentSettings).not.toHaveBeenCalled();
});

it('keeps Responses selected after applying compact settings and describes queue and interruption accurately', async () => {
  const user = userEvent.setup();
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'alice',
    ready: true,
    checked_at: '2026-11-03T10:00:00Z',
    services: [],
  });
  vi.spyOn(accessRequests, 'list').mockResolvedValue([]);
  const transport = harness();
  const settings = {
    model: 'gpt-6',
    reasoning_effort: 'high',
    permission_mode: 'autonomous',
  } as const;
  const update = vi.fn().mockResolvedValue({ ...settings, permission_mode: 'guarded' });
  const client = {
    ...transport.client,
    readAgentSettings: async () => settings,
    readAgentModels: async () => [{ id: 'gpt-6', label: 'GPT-6', available: true }],
    updateAgentSettings: update,
  };
  function Agent() {
    const controller = useAgentController(client, 'agt_1', { viewerId: 'alice' });
    return (
      <CmaClientProvider client={client}>
        <AgentSurface
          agentId="agt_1"
          viewerId="alice"
          controller={controller}
          detailsOpen={false}
          onDetailsOpenChange={() => undefined}
          onNavigate={() => undefined}
        />
      </CmaClientProvider>
    );
  }
  render(<Agent />);
  await waitFor(() => expect(transport.streaming).toBe(true));
  const field = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(field, { target: { value: 'Next message stays on Responses' } });
  const summary = await screen.findByRole('button', { name: /^Agent settings:/ });
  await user.click(summary);
  await user.click(screen.getByRole('button', { name: 'Permission mode' }));
  await user.click(screen.getByRole('menuitemradio', { name: 'Guarded' }));
  await user.click(summary);
  await user.click(screen.getByRole('button', { name: 'Apply settings' }));
  await screen.findByText('Settings applied to this Agent.');
  expect(update).toHaveBeenCalledWith(
    'agt_1',
    { ...settings, permission_mode: 'guarded' },
    expect.any(Object),
  );
  await act(async () => transport.durable(1, { type: 'turn_started', turn_sequence: 1 }, binding));
  await user.click(screen.getByRole('button', { name: 'Steering mode: Queue' }));
  expect(screen.getByText('Process after the current turn finishes.')).toBeVisible();
  expect(
    screen.getByText('Stop the current turn, then start a new turn with this message.'),
  ).toBeVisible();
  await user.keyboard('{Escape}');
  expect(screen.getByRole('button', { name: 'Stop response' })).toBeEnabled();
  await user.click(screen.getByRole('button', { name: 'Queue message' }));
  await waitFor(() => expect(transport.post).toHaveBeenCalledOnce());
  expect(JSON.parse(String(transport.post.mock.calls[0]![0]?.body)).input).toBe(
    'Next message stays on Responses',
  );
  expect(transport.client.runAgent).not.toHaveBeenCalled();
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(field);
});
