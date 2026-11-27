import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ActiveThinking } from './ActiveThinking';
import { conversationFromTranscript, foldAgentEvent, foldLiveEvent } from './conversation';
import type { ConversationState } from './model';
import { THINKING_NO_PROGRESS_THRESHOLD_MS as threshold } from './ThinkingProgress';

const at = '2026-11-01T00:00:00Z';
const start = () =>
  foldAgentEvent(conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] }), {
    seq: 1,
    appendedAt: at,
    kind: { type: 'turn_started', turn_sequence: 1 },
  });
const advance = (milliseconds: number) => act(() => vi.advanceTimersByTime(milliseconds));
const notice = () => screen.queryByText(/No new activity observed here/);
afterEach(() => vi.useRealTimers());

it('shows the phase and recorded progress without treating reconnects as activity', () => {
  const phaseStarted = '2026-11-01T00:01:00Z';
  const toolOutput = '2026-11-01T00:02:00Z';
  const control = (name: string) => screen.getByText(name).nextElementSibling;
  let state = start();
  const view = render(<ActiveThinking conversation={state} />);
  fireEvent.click(screen.getByRole('button', { name: 'View activity' }));
  expect(control('Current phase')).toHaveTextContent('Waiting for a response');
  expect(control('Phase started')?.querySelector('time')).toHaveAttribute('datetime', at);
  expect(control('Last recorded progress')).toHaveTextContent('Unavailable');

  state = foldAgentEvent(state, {
    seq: 2,
    appendedAt: phaseStarted,
    kind: { type: 'tool_call_started', call_id: 'tool', title: 'Read file', status: 'running' },
  });
  view.rerender(<ActiveThinking conversation={state} />);
  expect(control('Current phase')).toHaveTextContent('Running a tool');
  expect(control('Phase started')?.querySelector('time')).toHaveAttribute('datetime', phaseStarted);

  state = foldAgentEvent(state, {
    seq: 3,
    appendedAt: toolOutput,
    kind: {
      type: 'tool_output_reported',
      call_id: 'tool',
      output: { text: 'data', truncated: false },
    },
  });
  view.rerender(<ActiveThinking conversation={{ ...state, agentCursor: 'reconnected' }} />);
  expect(control('Phase started')?.querySelector('time')).toHaveAttribute('datetime', phaseStarted);
  expect(control('Last recorded progress')?.querySelector('time')).toHaveAttribute(
    'datetime',
    toolOutput,
  );
});

it('reports prolonged observed silence without changing accepted work or its tools', () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(at));
  const state = {
    ...start(),
    toolCalls: [{ id: 'long-tool', runId: '1', title: 'Long task', status: 'running' }],
  };
  const before = structuredClone(state);
  render(<ActiveThinking conversation={state} />);
  advance(threshold - 1);
  expect(notice()).not.toBeInTheDocument();
  advance(1);
  expect(notice()).toHaveTextContent('No new activity observed here for 5 minutes.');
  expect(screen.getByText('Thinking')).toBeInTheDocument();
  advance(60_000);
  expect(notice()).toHaveTextContent('6 minutes');
  expect(state).toEqual(before);
  expect(state.activeRunId).toBe('1');
  expect(state.turnOutcomes?.['1']).toBeUndefined();
});

it.each(['reply', 'reasoning', 'tool', 'plan'])(
  'resets on new %s activity but not equivalent snapshots or observation-only changes',
  (kind) => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(at));
    let state: ConversationState = start();
    const view = render(<ActiveThinking conversation={state} />);
    advance(threshold);
    expect(notice()).toBeInTheDocument();
    state =
      kind === 'reasoning'
        ? foldLiveEvent(
            state,
            {
              type: 'REASONING_MESSAGE_CONTENT',
              threadId: 'agent',
              runId: '1',
              messageId: 'thought',
              delta: 'New thought',
            },
            'agent',
          )
        : foldAgentEvent(state, {
            seq: 2,
            appendedAt: at,
            kind:
              kind === 'reply'
                ? { type: 'agent_message', content: 'New reply' }
                : kind === 'tool'
                  ? {
                      type: 'tool_call_started',
                      call_id: 'tool',
                      title: 'Read file',
                      status: 'running',
                    }
                  : { type: 'plan_set', entries: [{ title: 'Read files', status: 'in_progress' }] },
          });
    view.rerender(<ActiveThinking conversation={state} />);
    expect(notice()).not.toBeInTheDocument();
    advance(threshold - 60_000);
    view.rerender(
      <ActiveThinking conversation={{ ...structuredClone(state), agentCursor: 'reconnected' }} />,
    );
    advance(60_000);
    expect(notice()).toHaveTextContent('5 minutes');
  },
);

it('starts a new observation window on navigation or a new turn and disappears at terminal', () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-11-01T00:35:00Z'));
  let state = start();
  const view = render(<ActiveThinking conversation={state} />);
  expect(notice()).not.toBeInTheDocument();
  advance(threshold);
  expect(notice()).toBeInTheDocument();
  state = foldAgentEvent(state, {
    seq: 2,
    appendedAt: at,
    kind: { type: 'turn_started', turn_sequence: 2 },
  });
  view.rerender(<ActiveThinking conversation={state} />);
  expect(notice()).not.toBeInTheDocument();
  advance(threshold);
  expect(notice()).toBeInTheDocument();
  view.unmount();
  const reloaded = render(<ActiveThinking conversation={state} />);
  expect(notice()).not.toBeInTheDocument();
  vi.setSystemTime(Date.now() + threshold);
  fireEvent(document, new Event('visibilitychange'));
  expect(notice()).toBeInTheDocument();
  state = foldAgentEvent(state, {
    seq: 3,
    appendedAt: at,
    kind: { type: 'turn_completed', turn_sequence: 2 },
  });
  reloaded.rerender(<ActiveThinking conversation={state} />);
  advance(200);
  expect(notice()).not.toBeInTheDocument();
});
