import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { AgentSidePanel, type AgentPanel } from './AgentSidePanel';
import { CmaClientProvider } from '../api/context';
import { AGENT_DETAIL, agentClient } from '../test/agent';
import { ArtifactPublicationScope } from '../resources/artifactRecovery';
import { conversationFromTranscript } from './conversation';

const base = conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] });
const workingDirectory = '/home/cma/project/.worktrees/wt-fixture';

it.each([false, true])(
  'opens Computer and Details at the same conversation path (shared=%s)',
  async (shared) => {
    const user = userEvent.setup();
    const clipboard = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue();
    const path = `${workingDirectory.slice(6)}/note.md`;
    const files = {
      list: vi.fn(async () => [
        { path, name: 'note.md', kind: 'file' as const, size_bytes: 0, hidden: false },
      ]),
      read: vi.fn(async () => ({
        path,
        size_bytes: 0,
        sha256: 'a'.repeat(64),
        content_base64: '',
      })),
      write: vi.fn(),
      download: vi.fn(),
      remove: vi.fn(),
    };
    const resourceViews = createClient({
      baseUrl: 'https://fixture.invalid',
      fetch: async (input) =>
        Response.json(
          String(input).includes('/resource-access/')
            ? {
                capabilities: ['view_details', 'view_files'],
                policy_revision: 1,
                revalidate_at: null,
              }
            : {
                access: 'shared',
                id: 'agt_1',
                status: 'ready',
                created_at: '2026-09-01T00:00:00Z',
                updated_at: '2026-09-01T00:00:00Z',
              },
        ),
    });
    const client = agentClient({ files, ...(shared ? { resourceViews } : {}) });
    const view = (panel: AgentPanel) => (
      <CmaClientProvider client={client}>
        <AgentSidePanel
          agentId="agt_1"
          viewerId="viewer"
          shared={shared}
          detail={
            shared
              ? undefined
              : { ...AGENT_DETAIL, runtimeMetadata: { workingDirectory: '/home/detail-fallback' } }
          }
          conversation={{ ...base, runtimeMetadata: { workingDirectory } }}
          panel={panel}
          onClose={vi.fn()}
          onAccessChanged={vi.fn()}
          publicationScope={new ArtifactPublicationScope()}
        />
      </CmaClientProvider>
    );
    const { rerender } = render(view('files'));
    const computer = screen.getByRole('tabpanel', { name: 'Files' });
    await waitFor(() =>
      expect(files.list).toHaveBeenCalledWith(
        'agt_1',
        workingDirectory.slice(6),
        expect.any(Object),
      ),
    );
    expect(within(computer).getByLabelText('Working directory')).toHaveTextContent(
      workingDirectory,
    );
    rerender(view('details'));
    await user.click(await screen.findByRole('button', { name: 'Browse files' }));
    const dialog = screen.getByRole('dialog', { name: 'Files' });
    expect(within(dialog).getByLabelText('Working directory')).toHaveTextContent(workingDirectory);
    await user.click(await within(dialog).findByRole('button', { name: 'note.md' }));
    await user.click(within(dialog).getByRole('button', { name: 'Copy path' }));
    expect(clipboard).toHaveBeenCalledWith(`/home/${path}`);
    if (shared) {
      expect(within(dialog).queryByRole('button', { name: 'New file' })).toBeNull();
      expect(within(dialog).getByRole('textbox', { name: 'Contents of note.md' })).toHaveAttribute(
        'readonly',
      );
    }
    expect(files.write).not.toHaveBeenCalled();
    expect(files.remove).not.toHaveBeenCalled();
  },
);
