import { act, render, renderHook, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, agentClient } from '../test/agent';
import { useAgentController } from './controller';
import { SignInCard } from './SignInCard';
import { conversationFromTranscript, foldAgentEvent, foldLiveEvent } from './conversation';
import type { AgentEvent, AgentLiveEvent } from '../api/types';

afterEach(() => vi.useRealTimers());

it('polls a starting Agent until its first message is durably delivered without posting that message again', async () => {
  vi.useFakeTimers();
  let delivered = false;
  const readAgent = vi.fn(async () =>
    delivered
      ? AGENT_DETAIL
      : {
          ...AGENT_DETAIL,
          status: 'starting' as const,
          firstMessage: { state: 'queued' as const, queuedAt: AGENT_DETAIL.createdAt },
          readyAt: undefined,
        },
  );
  const createAgent = vi.fn();
  const runAgent = vi.fn();
  const client = agentClient({
    readAgent,
    createAgent,
    runAgent,
    readAgentTranscript: async () =>
      delivered ? AGENT_TRANSCRIPT : { agentId: 'agt_1', throughSeq: 0, turns: [] },
  });
  const { result, unmount } = renderHook(() => useAgentController(client, 'agt_1'));
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
  expect(result.current.state.status).toBe('ready');
  if (result.current.state.status === 'ready')
    expect(result.current.state.conversation.messages).toHaveLength(0);
  delivered = true;
  await act(async () => {
    await vi.advanceTimersByTimeAsync(250);
  });
  if (result.current.state.status === 'ready')
    expect(result.current.state.conversation.messages[0]?.content).toBe('Inspect this repository.');
  await act(async () => {
    await vi.advanceTimersByTimeAsync(10000);
  });
  expect(readAgent).toHaveBeenCalledTimes(4);
  expect(createAgent).not.toHaveBeenCalled();
  expect(runAgent).not.toHaveBeenCalled();
  unmount();
});

it('keeps startup reads sequential, slows prolonged preparation and stops on unmount', async () => {
  vi.useFakeTimers();
  const starting = { ...AGENT_DETAIL, status: 'starting' as const, readyAt: undefined };
  let finishRead: ((value: typeof starting) => void) | undefined;
  let readSignal: AbortSignal | undefined;
  const readAgent = vi.fn(async (_id: string, options?: { signal?: AbortSignal }) => {
    readSignal = options?.signal;
    if (readAgent.mock.calls.length === 1) return starting;
    return new Promise<typeof starting>((resolve) => {
      finishRead = resolve;
    });
  });
  const readAgentTranscript = vi.fn(async () => AGENT_TRANSCRIPT);
  const client = agentClient({ readAgent, readAgentTranscript });
  const { unmount } = renderHook(() => useAgentController(client, 'agt_1'));
  await act(() => vi.advanceTimersByTimeAsync(0));
  await act(() => vi.advanceTimersByTimeAsync(250));
  expect(readAgent).toHaveBeenCalledTimes(2);
  await act(() => vi.advanceTimersByTimeAsync(10000));
  expect(readAgent).toHaveBeenCalledTimes(2);
  expect(readAgentTranscript).toHaveBeenCalledTimes(1);
  await act(async () => finishRead?.(starting));
  await act(() => vi.advanceTimersByTimeAsync(1499));
  expect(readAgent).toHaveBeenCalledTimes(2);
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(readAgent).toHaveBeenCalledTimes(3);
  unmount();
  expect(readSignal?.aborted).toBe(true);
  await act(async () => finishRead?.(starting));
  await act(() => vi.advanceTimersByTimeAsync(10000));
  expect(readAgent).toHaveBeenCalledTimes(3);
});

it('rejects changed startup lineage before reading history and retains error backoff', async () => {
  vi.useFakeTimers();
  const starting = { ...AGENT_DETAIL, status: 'starting' as const, readyAt: undefined };
  const readAgent = vi
    .fn()
    .mockResolvedValueOnce(starting)
    .mockResolvedValue({
      ...starting,
      workspace: { ...starting.workspace, id: 'foreign-workspace' },
    });
  const readAgentTranscript = vi.fn(async () => AGENT_TRANSCRIPT);
  const client = agentClient({ readAgent, readAgentTranscript });
  const { result, unmount } = renderHook(() => useAgentController(client, 'agt_1'));
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(readAgent).toHaveBeenCalledTimes(2);
  expect(readAgentTranscript).toHaveBeenCalledTimes(1);
  expect(result.current.actionError).toBe(
    'Preparation progress could not be refreshed. Reconnecting.',
  );
  await act(() => vi.advanceTimersByTimeAsync(4999));
  expect(readAgent).toHaveBeenCalledTimes(2);
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(readAgent).toHaveBeenCalledTimes(3);
  expect(readAgentTranscript).toHaveBeenCalledTimes(1);
  unmount();
});

it('preserves live fragments and reasoning through advancing JSON polls and reconciles the durable message', async () => {
  vi.useFakeTimers();
  let onEvent: ((event: AgentLiveEvent) => void) | undefined;
  let events: AgentEvent[] = [
    {
      seq: 1,
      appendedAt: AGENT_DETAIL.createdAt,
      kind: { type: 'turn_started', turn_sequence: 1 },
    },
  ];
  const client = agentClient({
    readAgent: async () => ({
      ...AGENT_DETAIL,
      firstMessage: { state: 'execution_observed', queuedAt: AGENT_DETAIL.createdAt },
    }),
    readAgentTranscript: async () => ({ agentId: 'agt_1', throughSeq: 0, turns: [] }),
    readAgentEvents: async () => ({ items: events }),
    replayAgent: async (_id, options) => {
      onEvent = options.onEvent;
    },
  });
  const { result, unmount } = renderHook(() => useAgentController(client, 'agt_1'));
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
  await act(async () => {
    onEvent?.({
      type: 'TEXT_MESSAGE_CONTENT',
      threadId: 'agt_1',
      runId: '1',
      messageId: 'live-1',
      delta: 'Inspecting',
      'cma:live': true,
    });
    onEvent?.({
      type: 'REASONING_MESSAGE_CONTENT',
      threadId: 'agt_1',
      runId: '1',
      messageId: 'reason-1',
      delta: 'Reading the repository.',
    });
  });
  events = [
    ...events,
    { seq: 2, appendedAt: AGENT_DETAIL.createdAt, kind: { type: 'status_changed', to: 'busy' } },
  ];
  await act(async () => {
    await vi.advanceTimersByTimeAsync(1500);
  });
  if (result.current.state.status !== 'ready') throw new Error('Agent did not load');
  expect(result.current.state.conversation.messages).toMatchObject([
    { id: 'live-1', content: 'Inspecting' },
  ]);
  expect(result.current.state.conversation.reasoning[0]?.content).toBe('Reading the repository.');
  events = [
    ...events,
    {
      seq: 3,
      appendedAt: AGENT_DETAIL.createdAt,
      kind: { type: 'agent_message', content: 'Inspection complete.' },
    },
  ];
  await act(async () => {
    await vi.advanceTimersByTimeAsync(1500);
  });
  if (result.current.state.status !== 'ready') throw new Error('Agent did not load');
  expect(result.current.state.conversation.messages).toMatchObject([
    { id: 'event:3', content: 'Inspection complete.' },
  ]);
  expect(result.current.state.conversation.reasoning[0]?.content).toBe('Reading the repository.');
  unmount();
});

it('renders the own-account sign-in card with a safe link and copyable code', async () => {
  const user = userEvent.setup();
  const clipboard = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue();
  render(
    <SignInCard
      notice={{
        id: 'sign-in:1',
        title: 'Sign in required',
        detail: 'Authorize your account.',
        code: 'ABCD-EFGH',
        url: 'https://accounts.example.test/device',
      }}
    />,
  );
  expect(screen.getByText('Needs you')).toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'Continue sign in' })).toHaveAttribute(
    'rel',
    'noreferrer noopener',
  );
  await user.click(screen.getByRole('button', { name: 'Copy sign-in code' }));
  expect(clipboard).toHaveBeenCalledWith('ABCD-EFGH');
  expect(screen.getByRole('status')).toHaveTextContent('Copied');
});

it.each(['javascript:alert(1)', 'https://user:password@example.test', 'invalid'])(
  'refuses unsafe sign-in URL %s',
  (url) => {
    render(
      <SignInCard
        notice={{ id: 'sign-in:1', title: 'Sign in', detail: 'Sign in required.', url }}
      />,
    );
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
  },
);

it('retires sign-in notices when a turn starts or the live sign-in state clears', () => {
  const empty = conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] });
  const notice = foldAgentEvent(empty, {
    seq: 1,
    appendedAt: AGENT_DETAIL.createdAt,
    kind: { type: 'sign_in_required', message: 'Authorize your account.' },
  });
  expect(notice.recovery).toHaveLength(1);
  expect(
    foldAgentEvent(notice, {
      seq: 2,
      appendedAt: AGENT_DETAIL.createdAt,
      kind: { type: 'turn_started', turn_sequence: 2 },
    }).recovery,
  ).toHaveLength(0);
  expect(
    foldLiveEvent(
      notice,
      {
        type: 'STATE_DELTA',
        threadId: 'agt_1',
        runId: '2',
        delta: [{ op: 'replace', path: '/signIn', value: null }],
      },
      'agt_1',
    ).recovery,
  ).toHaveLength(0);
});

it.each([undefined, 'existing-run'])(
  'clears sign-in notices on conversation open while preserving run %s and unrelated recovery',
  (activeRunId) => {
    const history = conversationFromTranscript(AGENT_TRANSCRIPT);
    const signIn = {
      seq: 5,
      appendedAt: AGENT_DETAIL.createdAt,
      kind: { type: 'sign_in_required', message: 'Authorize your account.' },
    };
    const notice = foldAgentEvent(history, signIn);
    const liveNotice = foldLiveEvent(
      notice,
      {
        type: 'STATE_DELTA',
        threadId: 'agt_1',
        runId: '0',
        delta: [{ op: 'replace', path: '/signIn', value: { message: 'Authorize your account.' } }],
      },
      'agt_1',
    );
    const waiting = foldAgentEvent(
      { ...liveNotice, activeRunId },
      {
        seq: 6,
        appendedAt: AGENT_DETAIL.createdAt,
        kind: { type: 'first_message_changed', state: 'recovery_required' },
      },
    );
    const unrelated = waiting.recovery.find((entry) => entry.id === 'first-message-recovery');
    expect(waiting.recovery.map((entry) => entry.id)).toEqual([
      'sign-in:5',
      'live-sign-in',
      'first-message-recovery',
    ]);
    const opened = foldAgentEvent(waiting, {
      seq: 7,
      appendedAt: AGENT_DETAIL.createdAt,
      kind: {
        type: 'conversation_opened',
        conversation_id: 'conversation-ready',
        runtime_generation: 1,
      },
    });
    expect(opened.recovery).toEqual([unrelated]);
    expect(opened.activeRunId).toBe(activeRunId);
    expect(opened.messages).toEqual(waiting.messages);
    expect(opened.cursor).toBe(7);
    expect(foldAgentEvent(opened, signIn)).toEqual(opened);
  },
);
