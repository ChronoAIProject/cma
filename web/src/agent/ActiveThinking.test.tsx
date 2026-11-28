import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { MessageStream } from './MessageStream';
import {
  conversationFromTranscript,
  foldAgentEvent,
  foldLiveEvent,
  type ConversationState,
} from './conversation';
import { mergeTranscriptSnapshot } from './chronology';
import { agentClient } from '../test/agent';
import { externalApprovalFixture } from './externalApproval.fixture';

const startedAt = '2026-09-14T00:00:00Z';
const empty = () => conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] });
const live = (state: ConversationState, type: string, fields = {}) =>
  foldLiveEvent(
    state,
    {
      threadId: 'agent',
      runId: '1',
      timestamp: startedAt,
      type,
      ...fields,
    },
    'agent',
  );
const start = () => live(empty(), 'RUN_STARTED');
const draw = (conversation: ConversationState) => (
  <>
    <div data-testid="history">
      <MessageStream conversation={conversation} onApproval={vi.fn()} />
    </div>
  </>
);
afterEach(() => vi.useRealTimers());

it('keeps one current-turn picture through tools and approval until normal completion', () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-01-01T00:04:59.000Z'));
  const client = agentClient();
  const acceptance = {
    task_id: 'task',
    turn_sequence: 1,
    verdict: { status: 'succeeded' as const },
    unconfirmed_outputs: [],
  };
  const branded = (
    conversation: ConversationState,
    taskAcceptance?: typeof acceptance,
    authEpoch = 1,
  ) => (
    <MessageStream
      conversation={conversation}
      retainedOutput={{ client, agentId: 'agent', viewerId: 'viewer' }}
      turnPresentation={{ status: 'ready', authEpoch }}
      taskAcceptance={taskAcceptance}
      onApproval={vi.fn()}
    />
  );
  const reason = live(start(), 'REASONING_MESSAGE_CONTENT', {
    messageId: 'idea',
    delta: 'Inspecting',
  });
  const working = live(reason, 'TOOL_CALL_START', { toolCallId: 'tool', toolCallName: 'Read' });
  const oldApproval = { ...externalApprovalFixture, id: 'old-approval', runId: '0' };
  const waiting = { ...working, approvals: [oldApproval, externalApprovalFixture] };
  const ended = {
    ...waiting,
    activeRunId: undefined,
    latestTurnId: '1',
    turnOutcomes: { 1: 'completed' as const },
  };
  const { container, rerender } = render(branded(reason));
  const progress = () => within(container.querySelector<HTMLElement>('.turn-activity')!);
  const art = container.querySelector('.turn-activity .agent-mascot__artwork');
  expect(progress().getByText('Thinking')).toBeVisible();
  rerender(branded({ ...working, approvals: [oldApproval] }, acceptance));
  expect(progress().getByText('Running a tool')).toBeVisible();
  rerender(branded(waiting, acceptance));
  expect(progress().getByText('Waiting for approval')).toBeVisible();
  act(() => vi.advanceTimersByTime(1000));
  expect(
    progress().getByText('Waiting for the server to confirm the approval outcome'),
  ).toBeVisible();
  expect(screen.getByRole('heading', { name: 'Waiting for your approval' })).toBeVisible();
  const completionStatus = container.querySelector('.sr-only[role="status"]');
  expect(completionStatus).toBeEmptyDOMElement();
  rerender(branded(ended));
  expect(completionStatus).toHaveTextContent('Turn completed.');
  expect(container.querySelector('.sr-only[role="status"]')).toBe(completionStatus);
  expect(screen.queryByText('Turn ended', { exact: true })).not.toBeInTheDocument();
  rerender(branded(ended, acceptance));
  expect(container.querySelector('.turn-activity .agent-mascot__artwork')).toBeNull();
  expect(art).not.toBeInTheDocument();
  rerender(branded({ ...working, activeRunId: '2', latestTurnId: '2' }, acceptance, 2));
  expect(progress().getByText('Waiting for a response')).toBeVisible();
});

it('keeps Thinking below new text and moves concurrent tools into separate completion-ordered cards', () => {
  let state = live(start(), 'REASONING_MESSAGE_CONTENT', {
    messageId: 'reason',
    delta: 'Inspecting dependencies',
  });
  state = live(state, 'TOOL_CALL_START', { toolCallId: 'slow', toolCallName: 'Run tests' });
  state = live(state, 'TOOL_CALL_START', { toolCallId: 'fast', toolCallName: 'Read package' });
  const { rerender } = render(draw(state));
  const toggle = screen.getByRole('button', { name: 'View activity' });
  expect(toggle).toHaveAttribute('aria-expanded', 'false');
  expect(toggle).toHaveTextContent('View activity');
  expect(toggle).not.toHaveTextContent('Turn input unavailable');
  expect(toggle).not.toHaveTextContent('Inspecting dependencies');
  // Pending tools put the turn's character to work (§28).
  const logo = () => document.querySelector('.thinking__logo');
  expect(logo()).toHaveAttribute('aria-hidden', 'true');
  fireEvent.click(toggle);
  const dock = screen.getByRole('region', { name: 'What the agent is working through' });
  expect(within(dock).getByText('Inspecting dependencies')).toBeInTheDocument();
  expect(within(dock).getAllByRole('listitem')).toHaveLength(2);
  expect(screen.queryByRole('article', { name: 'Tool call: Run tests' })).not.toBeInTheDocument();
  expect(screen.getByTestId('history')).toContainElement(dock);

  state = live(state, 'TEXT_MESSAGE_CONTENT', {
    messageId: 'update',
    'cma:live': true,
    delta: 'The first check is ready.',
  });
  state = live(state, 'TOOL_CALL_END', { toolCallId: 'fast', result: 'package.json read' });
  rerender(draw(state));
  expect(screen.getByRole('button', { name: 'View activity' })).toBe(toggle);
  expect(within(dock).getAllByRole('listitem')).toHaveLength(1);
  expect(within(dock).queryByText(/Read package/)).not.toBeInTheDocument();
  const completed = screen.getByRole('button', { name: /^Read package/ });
  expect(completed).toHaveAttribute('aria-expanded', 'false');
  fireEvent.click(completed);
  expect(
    within(screen.getByRole('article', { name: 'Tool call: Read package' })).getByRole('img', {
      name: 'Completed',
    }),
  ).toBeInTheDocument();
  expect(
    screen.getByText('The first check is ready.').compareDocumentPosition(dock) &
      Node.DOCUMENT_POSITION_FOLLOWING,
  ).toBeTruthy();

  state = live(state, 'TOOL_CALL_END', {
    toolCallId: 'slow',
    'cma:status': 'failed',
    result: 'One assertion failed',
  });
  rerender(draw(state));
  // With no tool left pending and reply text streaming, the character replies.
  expect(logo()).toHaveAttribute('aria-hidden', 'true');
  const cards = screen.getAllByRole('article', { name: /^Tool call:/ });
  expect(cards.map((card) => card.getAttribute('aria-label'))).toEqual([
    'Tool call: Read package',
    'Tool call: Run tests',
  ]);
  expect(cards[1]).toHaveTextContent('One assertion failed');
  expect(screen.getByRole('button', { name: /Read package · Run tests/ })).toHaveAttribute(
    'aria-expanded',
    'true',
  );
  fireEvent.click(within(cards[1]!).getByRole('button', { name: /Run tests/ }));
  expect(within(cards[1]!).getAllByText('One assertion failed')[0]).toBeVisible();
  expect(within(dock).queryByRole('list')).not.toBeInTheDocument();
  expect(toggle).toHaveAttribute('aria-expanded', 'true');

  vi.useFakeTimers();
  rerender(draw(live(state, 'RUN_FINISHED')));
  expect(screen.queryByRole('button', { name: 'View activity' })).not.toBeInTheDocument();
  act(() => vi.advanceTimersByTime(200));
  expect(screen.getAllByRole('article', { name: /^Tool call:/ })).toHaveLength(2);
  expect(screen.queryByText('Inspecting dependencies')).not.toBeInTheDocument();
});

it('shows exactly one transient row before the first reply and before queued follow-up messages', () => {
  const state = start();
  const { container } = render(
    <MessageStream
      conversation={state}
      onApproval={vi.fn()}
      optimisticMessages={[
        { id: 'queued', content: 'Next question', state: 'accepted', queued: true },
      ]}
    />,
  );
  const thinking = screen.getByRole('region', { name: 'What the agent is working through' });
  expect(thinking.closest('.message-stream')).not.toBeNull();
  expect(thinking.closest('article')).toBeNull();
  expect(container.querySelectorAll('.active-thinking')).toHaveLength(1);
  // Before any tool or reply, the character thinks.
  expect(thinking.querySelector('.thinking__logo')).toHaveAttribute('aria-hidden', 'true');
  expect(
    thinking.compareDocumentPosition(screen.getByText('Next question')) &
      Node.DOCUMENT_POSITION_FOLLOWING,
  ).toBeTruthy();
  expect(container.querySelector('.agent-surface__input .thinking')).toBeNull();
});

it('restores elapsed time on navigation and replay, and resets it only for a new turn', () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-09-14T00:02:00Z'));
  const active = start();
  const first = render(draw(active));
  expect(screen.getByText('2m 0s')).toBeInTheDocument();
  act(() => vi.advanceTimersByTime(3_000));
  expect(screen.getByText('2m 3s')).toBeInTheDocument();
  first.unmount();
  act(() => vi.advanceTimersByTime(7_000));
  const next = render(draw(live(active, 'RUN_STARTED', { timestamp: '2026-09-14T00:02:10Z' })));
  expect(screen.getByText('2m 10s')).toBeInTheDocument();
  next.rerender(
    draw(live(active, 'RUN_STARTED', { runId: '2', timestamp: '2026-09-14T00:02:10Z' })),
  );
  expect(screen.getByText('0s')).toBeInTheDocument();
  next.rerender(draw({ ...active, activeRunStartedAt: undefined }));
  expect(document.querySelector('.thinking__elapsed')).toBeNull();
});

it.each(['RUN_FINISHED', 'RUN_ERROR'])(
  'removes inline Thinking on %s without inventing a pending tool outcome',
  (end) => {
    const pending = live(start(), 'TOOL_CALL_START', {
      toolCallId: 'pending',
      toolCallName: 'Long task',
    });
    render(draw(live(pending, end)));
    expect(screen.queryByRole('button', { name: 'View activity' })).not.toBeInTheDocument();
    const process = screen.getByRole('button', { name: 'Long task' });
    expect(process).toHaveAttribute('aria-expanded', 'false');
    fireEvent.click(process);
    expect(
      within(screen.getByRole('article', { name: 'Tool call: Long task' })).getByRole('img', {
        name: 'Outcome not reported (last status: running)',
      }),
    ).toBeInTheDocument();
  },
);

it('uses the active transcript start on reload and never pairs a stale timestamp with a newer turn', () => {
  const snapshot = conversationFromTranscript({
    agentId: 'agent',
    throughSeq: 8,
    turns: [
      {
        turnSequence: 1,
        startedAt,
        ownerMessage: '',
        agentMessages: [],
        messages: [],
        outcome: { type: 'active' },
      },
    ],
  });
  expect(snapshot.activeRunStartedAt).toBe(startedAt);
  const current = live(snapshot, 'RUN_STARTED', {
    runId: '9',
    cursor: 9,
    timestamp: '2026-09-14T00:10:00Z',
  });
  expect(mergeTranscriptSnapshot(current, snapshot)).toMatchObject({
    activeRunId: '9',
    activeRunStartedAt: '2026-09-14T00:10:00Z',
  });
  expect(
    foldAgentEvent(current, { seq: 10, appendedAt: startedAt, kind: { type: 'turn_completed' } })
      .activeRunStartedAt,
  ).toBeUndefined();
});

it('reconciles terminal tools without duplicates or pending regression during replay', () => {
  let state = live(start(), 'TOOL_CALL_START', { toolCallId: 'call', toolCallName: 'Read file' });
  state = live(state, 'TOOL_CALL_END', { toolCallId: 'call' });
  const position = state.toolCalls[0]?.order;
  state = live(state, 'TOOL_CALL_START', { toolCallId: 'call', toolCallName: 'Read file' });
  state = live(state, 'TOOL_CALL_END', { toolCallId: 'call', 'cma:status': 'in_progress' });
  state = foldAgentEvent(state, {
    seq: 5,
    appendedAt: startedAt,
    kind: { type: 'tool_call_updated', call_id: 'call', status: 'completed' },
  });
  expect(state.toolCalls).toHaveLength(1);
  expect(state.toolCalls[0]).toMatchObject({ status: 'completed', order: position });
  render(draw(state));
  const process = screen.getByRole('button', { name: /^Read file/ });
  expect(process).toHaveAttribute('aria-expanded', 'false');
  fireEvent.click(process);
  expect(screen.getAllByRole('article', { name: 'Tool call: Read file' })).toHaveLength(1);
  fireEvent.click(screen.getByRole('button', { name: 'View activity' }));
  expect(screen.queryByRole('listitem')).not.toBeInTheDocument();
});
