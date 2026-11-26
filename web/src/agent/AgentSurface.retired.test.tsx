import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, agentClient } from '../test/agent';
import { workspaceFixture } from '../test/hierarchy';
import { AgentSurface } from './AgentSurface';
import type { AgentSurfaceProps } from './AgentSurfaceProps';
import { controller } from './AgentSurface.fixture';
import { conversationFromTranscript } from './conversation';
import { useAgentController } from './controller';
import { downloadAgentHistory } from './historyExport';
import * as connections from '../connections/preparation';
import * as suggestions from './useAgentSuggestions';
import * as firstSend from './useFirstTextSend';
import type { SelectedRetiredAgent } from '../useDeletionNoticeReconciliation';

vi.mock('./historyExport', () => ({ downloadAgentHistory: vi.fn() }));
afterEach(() => vi.restoreAllMocks());
const retired: SelectedRetiredAgent = {
  viewerId: 'owner',
  authEpoch: 2,
  detail: { ...AGENT_DETAIL, sandbox: { ...AGENT_DETAIL.sandbox, status: 'retired' } },
};
const conversation = conversationFromTranscript(AGENT_TRANSCRIPT);
const identity = {
  viewerId: 'owner',
  authEpoch: 2,
  agentId: 'agt_1',
  worktreeId: 'wkt_1',
  workspaceId: 'wsp_1',
  sandboxId: 'sbx_1',
};
function fixture(): AgentSurfaceProps {
  return {
    agentId: 'agt_1',
    viewerId: 'owner',
    selectedRetired: retired,
    accessAuthorized: true,
    firstSendContext: { authEpoch: 2, visible: true },
    detailsOpen: false,
    onDetailsOpenChange: vi.fn(),
    onNavigate: vi.fn(),
    permissions: {
      deleteSandbox: true,
      createWorkspace: true,
      createAgent: true,
      pauseSandbox: true,
      removeWorkspace: true,
      stopAgent: true,
      deleteAgent: true,
      exportHistory: true,
    },
    controller: controller(
      { status: 'ready', detail: AGENT_DETAIL, workspace: workspaceFixture(), conversation },
      undefined,
      {
        actionError: 'Old runtime connection failed',
        retained: {
          status: 'ready',
          detail: retired.detail,
          identity,
          conversation,
          retry: vi.fn(),
        },
      },
    ),
    lifecycleClient: agentClient(),
  };
}

it('chooses the retained host before active hooks and removes mutation/connection controls', async () => {
  const props = fixture();
  const connect = vi.spyOn(connections, 'readAgentConnections');
  const request = vi.spyOn(props.lifecycleClient!, 'runAgent');
  const suggestionHook = vi.spyOn(suggestions, 'useAgentSuggestions');
  const firstSendHook = vi.spyOn(firstSend, 'useFirstTextSend');
  const view = render(
    <CmaClientProvider client={props.lifecycleClient!}>
      <AgentSurface {...props} />
    </CmaClientProvider>,
  );
  expect(
    screen.getAllByText(
      "This agent's cloud computer was deleted. The conversation is kept read-only.",
    ),
  ).toHaveLength(1);
  expect(screen.getByRole('heading', { name: 'Agent one' })).toHaveFocus();
  expect(screen.getByText('The repository is ready.')).toBeVisible();
  for (const label of [
    'Send message',
    'Retry connections',
    'Stop Agent',
    'More Agent actions',
    'Helpful',
    'Needs improvement',
  ]) {
    expect(screen.queryByRole('button', { name: label })).not.toBeInTheDocument();
  }
  expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
  expect(screen.queryByText('Old runtime connection failed')).not.toBeInTheDocument();
  expect(view.container.querySelector('[data-motion="live"]')).toBeNull();
  await act(async () => {});
  expect(connect).not.toHaveBeenCalled();
  expect(suggestionHook).not.toHaveBeenCalled();
  expect(firstSendHook).not.toHaveBeenCalled();
  expect(request).not.toHaveBeenCalled();
  const read = vi.spyOn(props.lifecycleClient!, 'readAgentTranscript');
  fireEvent.click(screen.getByRole('button', { name: 'Export conversation' }));
  await waitFor(() => expect(downloadAgentHistory).toHaveBeenCalled());
  expect(read).toHaveBeenCalledExactlyOnceWith(
    'agt_1',
    expect.objectContaining({ cache: 'no-store', maxPages: 256, maxBytes: 16 * 1024 * 1024 }),
  );
});

it.each(['viewer', 'epoch', 'access', 'lineage'] as const)(
  'hides retained content on %s mismatch without mounting an active host',
  async (mismatch) => {
    const props = fixture();
    const connect = vi.spyOn(connections, 'readAgentConnections');
    const view = render(<AgentSurface {...props} />);
    const next = { ...props };
    if (mismatch === 'viewer') next.viewerId = 'other';
    if (mismatch === 'epoch') next.firstSendContext = { authEpoch: 3, visible: true };
    if (mismatch === 'access') next.accessUncertain = true;
    if (mismatch === 'lineage')
      next.selectedRetired = {
        ...retired,
        detail: { ...retired.detail, worktree: { ...retired.detail.worktree, agentId: 'other' } },
      };
    view.rerender(<AgentSurface {...next} />);
    expect(screen.queryByText('The repository is ready.')).not.toBeInTheDocument();
    expect(screen.queryByText(/cloud computer was deleted/)).not.toBeInTheDocument();
    expect(screen.getByText('Checking access to the retained conversation.')).toBeVisible();
    await act(async () => {});
    expect(connect).not.toHaveBeenCalled();
  },
);

it('shows independent history failure and explicit read recovery without runtime recovery', async () => {
  const props = fixture();
  const read = vi
    .fn()
    .mockRejectedValueOnce(new Error('Synthetic read unavailable'))
    .mockResolvedValue(AGENT_TRANSCRIPT);
  const client = agentClient({ readAgentTranscript: read });
  const runtime = vi.spyOn(client, 'replayAgent');
  function View() {
    const agent = useAgentController(client, 'agt_1', {
      viewerId: 'owner',
      authEpoch: 2,
      selectedRetired: retired,
    });
    return <AgentSurface {...props} lifecycleClient={client} controller={agent} />;
  }
  render(<View />);
  expect(await screen.findByText('Conversation couldn’t be loaded')).toBeVisible();
  expect(screen.getByText(/cloud computer was deleted/)).toBeVisible();
  expect(screen.queryByRole('button', { name: 'Retry connections' })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Load conversation again' }));
  expect(await screen.findByText('The repository is ready.')).toBeVisible();
  expect(read).toHaveBeenCalledTimes(2);
  expect(runtime).not.toHaveBeenCalled();
});

it('does not present an active turn as thinking or rewrite its recorded outcome', () => {
  const props = fixture();
  const active = { ...conversation, activeRunId: '1', activeRunStartedAt: '2026-09-06T00:01:00Z' };
  const agent = {
    ...props.controller,
    retained: {
      status: 'ready' as const,
      detail: retired.detail,
      identity,
      conversation: active,
      retry: vi.fn(),
    },
  };
  const view = render(<AgentSurface {...props} controller={agent} />);
  expect(view.container.querySelector('.active-thinking')).toBeNull();
  expect(view.container.querySelector('[data-motion="live"]')).toBeNull();
  expect(active.activeRunId).toBe('1');
  expect(active.turnOutcomes).toEqual(conversation.turnOutcomes);
});
