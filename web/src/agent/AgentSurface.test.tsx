import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, AGENT_USAGE, agentClient } from '../test/agent';
import { ApiError } from '@chronoai/cma-sdk';
import { CmaClientProvider } from '../api/context';
import { workspaceFixture } from '../test/hierarchy';
import { conversationFromTranscript, type ConversationState } from './conversation';
import type { AgentController, AgentResourceState } from './controller';
import { useAgentController } from './controller';
import { AgentSurface } from './AgentSurface';

it('shows authorized history while later pages load and preserves the editable draft when hydration finishes', async () => {
  let finish!: (page: { items: [] }) => void;
  const laterPage = new Promise<{ items: [] }>((resolve) => {
    finish = resolve;
  });
  const client = agentClient({
    readAgentEvents: vi
      .fn()
      .mockResolvedValueOnce({ items: [], next: 2 })
      .mockReturnValueOnce(laterPage),
  });
  function HydratingAgent() {
    const controller = useAgentController(client, 'agt_1');
    return (
      <AgentSurface
        agentId="agt_1"
        controller={controller}
        detailsOpen={false}
        onDetailsOpenChange={vi.fn()}
        onNavigate={vi.fn()}
      />
    );
  }
  render(
    <CmaClientProvider client={client}>
      <HydratingAgent />
    </CmaClientProvider>,
  );
  expect(await screen.findByText('The repository is ready.')).toBeInTheDocument();
  expect(
    screen.getByText('Loading conversation activity.').closest('[role="status"]'),
  ).toBeInTheDocument();
  const draft = screen.getByRole('textbox', { name: 'Message' });
  expect(draft).toBeEnabled();
  fireEvent.change(draft, { target: { value: 'Keep this draft' } });
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  await act(async () => finish({ items: [] }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled());
  expect(
    screen.getByText('Loading conversation activity.').closest('[role="status"]'),
  ).toHaveAttribute('inert');
  await waitFor(() =>
    expect(screen.queryByText('Loading conversation activity.')).not.toBeInTheDocument(),
  );
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(draft);
  expect(draft).toHaveValue('Keep this draft');
  expect(screen.getAllByText('The repository is ready.')).toHaveLength(1);
});

it('preserves a draft and visible history while incomplete event activity retries without a reload', async () => {
  vi.useFakeTimers();
  let unavailable = true;
  const readAgentEvents = vi.fn(async () => {
    if (unavailable)
      throw new ApiError(503, 'Authentication authority is temporarily unavailable.');
    return { items: [] };
  });
  const client = agentClient({ readAgentEvents });
  function RecoveringAgent() {
    const controller = useAgentController(client, 'agt_1');
    return (
      <>
        <button onClick={controller.reload}>Reload conversation</button>
        <AgentSurface
          agentId="agt_1"
          controller={controller}
          detailsOpen={false}
          onDetailsOpenChange={vi.fn()}
          onNavigate={vi.fn()}
        />
      </>
    );
  }
  const view = render(
    <CmaClientProvider client={client}>
      <RecoveringAgent />
    </CmaClientProvider>,
  );
  try {
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(screen.getByText('The repository is ready.')).toBeInTheDocument();
    expect(
      screen
        .getByText('Conversation activity is unavailable. Reconnecting.')
        .closest('[role="status"]'),
    ).toBeInTheDocument();
    const draft = screen.getByRole('textbox', { name: 'Message' });
    expect(draft).toBeEnabled();
    fireEvent.change(draft, { target: { value: 'Retain this unsubmitted draft' } });
    expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
    unavailable = false;
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(
      screen
        .getByText('Conversation activity is unavailable. Reconnecting.')
        .closest('[role="status"]'),
    ).toHaveAttribute('inert');
    expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
    await act(() => vi.advanceTimersByTimeAsync(200));
    expect(
      screen.queryByText('Conversation activity is unavailable. Reconnecting.'),
    ).not.toBeInTheDocument();
    expect(draft).toHaveValue('Retain this unsubmitted draft');
    expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
    expect(screen.getAllByText('The repository is ready.')).toHaveLength(1);
    unavailable = true;
    fireEvent.click(screen.getByRole('button', { name: 'Reload conversation' }));
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(screen.getByText('The repository is ready.')).toBeInTheDocument();
    expect(
      screen.getByText('Conversation activity is unavailable. Reconnecting.'),
    ).toBeInTheDocument();
  } finally {
    view.unmount();
    vi.useRealTimers();
  }
});

function controller(
  state: AgentResourceState,
  conversation?: ConversationState,
  overrides: Partial<AgentController> = {},
): AgentController {
  return {
    inputAvailability: { allowed: true },
    resourceAvailability: { state: 'live', runtimeVersion: 'runtime-one' },
    state:
      state.status === 'ready' && conversation !== undefined ? { ...state, conversation } : state,
    reload: vi.fn(),
    clearActionError: vi.fn(),
    send: vi.fn().mockResolvedValue(undefined),
    interrupt: vi.fn().mockResolvedValue(undefined),
    approve: vi.fn().mockResolvedValue(undefined),
    stop: vi.fn().mockResolvedValue(undefined),
    delete: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  };
}

it('holds approval and Stop response controls during history hydration', async () => {
  const conversation: ConversationState = {
    ...conversationFromTranscript(AGENT_TRANSCRIPT),
    activeRunId: 'turn:2',
    approvals: [{ id: 'approval_1', reason: 'cma:write', message: 'Apply the patch' }],
  };
  render(
    <AgentSurface
      agentId="agt_1"
      controller={controller({
        status: 'ready',
        detail: { ...AGENT_DETAIL, status: 'working' },
        workspace: workspaceFixture(),
        conversation,
        historyLoading: true,
      })}
      detailsOpen={false}
      onDetailsOpenChange={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
  expect(await screen.findByText('Resources unavailable')).toBeInTheDocument();
  expect(screen.getByRole('textbox', { name: 'Message' })).toBeEnabled();
  expect(screen.getByRole('button', { name: 'Allow' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Stop response' })).toBeDisabled();
});

it('holds approvals while saving and distinguishes pending admission from an actual active turn', async () => {
  const conversation: ConversationState = {
    ...conversationFromTranscript(AGENT_TRANSCRIPT),
    approvals: [{ id: 'approval_1', reason: 'cma:write', message: 'Apply the patch' }],
  };
  const approve = vi.fn();
  const interrupt = vi.fn();
  const draw = (overrides: Partial<AgentController>, activeRunId?: string) => (
    <AgentSurface
      agentId="agt_1"
      controller={controller(
        {
          status: 'ready',
          detail: { ...AGENT_DETAIL, status: 'working' },
          workspace: workspaceFixture(),
          conversation: { ...conversation, activeRunId },
        },
        undefined,
        { approve, interrupt, ...overrides },
      )}
      detailsOpen={false}
      onDetailsOpenChange={vi.fn()}
      onNavigate={vi.fn()}
    />
  );
  const inputAvailability = {
    allowed: false,
    notice: 'Saving progress. Messages will be available when saving finishes.',
    savingStartedAt: '2026-09-08T00:00:00Z',
  };
  const { rerender } = render(draw({ inputAvailability }));
  expect(await screen.findByText('Resources unavailable')).toBeInTheDocument();
  const draft = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(draft, { target: { value: 'Keep my draft' } });
  expect(draft).toBeEnabled();
  expect(screen.getByRole('button', { name: 'Allow' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  expect(screen.queryByRole('button', { name: 'Stop response' })).not.toBeInTheDocument();
  rerender(draw({ pending: 'send', inputAvailability: { allowed: true } }));
  expect(screen.queryByText('Starting.', { exact: true })).not.toBeInTheDocument();
  expect(document.querySelector('.agent-header__status')).toHaveTextContent('Working');
  expect(screen.queryByRole('button', { name: 'Stop response' })).not.toBeInTheDocument();
  expect(draft).toHaveValue('Keep my draft');
  rerender(draw({ pending: 'send', inputAvailability }, 'run-active'));
  expect(screen.getByRole('button', { name: 'Stop response' })).toBeEnabled();
  fireEvent.click(screen.getByRole('button', { name: 'Stop response' }));
  expect(interrupt).toHaveBeenCalledOnce();
  expect(approve).not.toHaveBeenCalled();
  expect(screen.getByText('The repository is ready.')).toBeInTheDocument();
});

it('focuses the route heading while loading, then the composer for a usable Agent', async () => {
  const { rerender } = render(
    <AgentSurface
      agentId="agt_1"
      controller={controller({ status: 'loading' })}
      detailsOpen={false}
      onDetailsOpenChange={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
  expect(screen.getByRole('heading', { name: 'Agent' })).toHaveFocus();

  const conversation = { ...conversationFromTranscript(AGENT_TRANSCRIPT), usage: AGENT_USAGE };
  rerender(
    <AgentSurface
      agentId="agt_1"
      controller={controller({
        status: 'ready',
        detail: AGENT_DETAIL,
        workspace: workspaceFixture(),
        conversation,
      })}
      detailsOpen={false}
      onDetailsOpenChange={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
  expect(screen.getByRole('textbox', { name: 'Message' })).toHaveFocus();
  expect(await screen.findByText('Resources unavailable')).toBeInTheDocument();
});

it('names the full lineage and exposes messages, approvals, tools, reasoning, usage, and details', async () => {
  const onDetailsOpenChange = vi.fn();
  const approve = vi.fn().mockResolvedValue(undefined);
  const actor = userEvent.setup();
  const base = conversationFromTranscript(AGENT_TRANSCRIPT);
  const conversation: ConversationState = {
    ...base,
    usage: AGENT_USAGE,
    toolCalls: [{ id: 'call_1', title: 'Typecheck', kind: 'shell', status: 'completed' }],
    reasoning: [{ id: 'why_1', content: 'Check the types.', status: 'complete' }],
    approvals: [{ id: 'approval_1', reason: 'cma:write', message: 'Apply the patch' }],
    recovery: [{ id: 'gap_1', title: 'Conversation recovery', detail: 'One frame was recovered.' }],
  };
  const ready = controller(
    { status: 'ready', detail: AGENT_DETAIL, workspace: workspaceFixture(), conversation },
    undefined,
    {
      approve,
    },
  );
  render(
    <AgentSurface
      agentId="agt_1"
      controller={ready}
      detailsOpen={false}
      onDetailsOpenChange={onDetailsOpenChange}
      onNavigate={vi.fn()}
    />,
  );
  expect(screen.getByRole('navigation', { name: 'Agent location' })).toHaveTextContent(
    'Atlas/chrono-bot/Agent',
  );
  expect(screen.getByRole('heading', { name: 'Agent one' })).toBeInTheDocument();
  expect(screen.getByText('The repository is ready.')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /^Thinking/ })).not.toBeInTheDocument();
  expect(screen.getByRole('article', { name: 'Tool call: Typecheck' })).toHaveTextContent(
    'shell · completed',
  );
  expect(screen.getByRole('article', { name: 'Tool call: Typecheck' })).toHaveTextContent(
    'Duration not reported',
  );
  expect(screen.queryByText('Check the types.')).not.toBeInTheDocument();
  expect(screen.getByText('Conversation recovery')).toBeInTheDocument();
  expect(screen.getByLabelText('Agent usage')).toHaveTextContent('120 input');
  await actor.click(screen.getByRole('button', { name: 'Allow' }));
  expect(approve).toHaveBeenCalledWith('approval_1', 'allow');
  await actor.click(screen.getByRole('button', { name: 'Show Agent details' }));
  expect(onDetailsOpenChange).toHaveBeenCalledWith(true);
});

it('removes completed preparation while preserving active and failed preparation', async () => {
  const conversation = conversationFromTranscript(AGENT_TRANSCRIPT);
  const draw = (detail: typeof AGENT_DETAIL) => (
    <AgentSurface
      agentId="agt_1"
      controller={controller({
        status: 'ready',
        detail,
        workspace: workspaceFixture(),
        conversation,
        sandboxEvents: [],
      })}
      detailsOpen={false}
      onDetailsOpenChange={vi.fn()}
      onNavigate={vi.fn()}
    />
  );
  const { rerender } = render(draw(AGENT_DETAIL));
  expect(await screen.findByText('Resources unavailable')).toBeInTheDocument();
  expect(screen.queryByRole('region', { name: 'Agent preparation' })).not.toBeInTheDocument();
  rerender(
    draw({
      ...AGENT_DETAIL,
      status: 'starting',
      readyAt: undefined,
      worktree: { ...AGENT_DETAIL.worktree, status: 'creating' },
    }),
  );
  expect(screen.getByRole('region', { name: 'Agent preparation' })).toHaveTextContent(
    'In progress',
  );
  rerender(
    draw({ ...AGENT_DETAIL, failure: { class: 'preparation', detail: 'Preparation failed' } }),
  );
  expect(screen.getByRole('region', { name: 'Agent preparation' })).toHaveTextContent(
    'Preparation failed',
  );
});
import '../test/connectedAccounts';
