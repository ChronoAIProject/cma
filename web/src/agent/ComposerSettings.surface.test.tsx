import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { connectionClient } from '../connections/client';
import { agentClient } from '../test/agent';
import { AgentSurface } from './AgentSurface';
import { useAgentController } from './controller';
import { accessRequests } from './accessRequests';

afterEach(() => vi.restoreAllMocks());

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
