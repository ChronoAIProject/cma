import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ActiveThinking } from './ActiveThinking';
import { conversationFromTranscript, foldAgentEvent, foldLiveEvent } from './conversation';
import { hasVisibleTurnProgress } from './ThinkingProgress';

const at = '2026-01-01T00:00:00.000Z';
const waiting = () =>
  foldAgentEvent(conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] }), {
    seq: 1,
    appendedAt: at,
    kind: { type: 'turn_started', turn_sequence: 1 },
  });
afterEach(() => vi.useRealTimers());

it('shows a recorded waiting start immediately on reload, without claiming a cause or terminal', () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-01-01T01:23:32Z'));
  const state = waiting();
  const before = structuredClone(state);
  const view = render(<ActiveThinking conversation={state} />);
  expect(screen.getByText('Waiting for a response')).toBeVisible();
  expect(screen.getByText(/Still waiting for this turn to finish/)).toBeVisible();
  expect(view.container.querySelector('time')).toHaveAttribute('datetime', at);
  expect(screen.queryByText('Thinking')).toBeNull();
  expect(document.body).not.toHaveTextContent(/approval|timed out|has ended|stuck/i);
  fireEvent.focus(window);
  act(() => vi.advanceTimersByTime(600_000));
  view.rerender(
    <ActiveThinking conversation={{ ...structuredClone(state), agentCursor: 'reconnected' }} />,
  );
  expect(view.container.querySelector('time')).toHaveAttribute('datetime', at);
  view.unmount();
  const next = render(<ActiveThinking conversation={state} />);
  expect(next.container.querySelector('time')).toHaveAttribute('datetime', at);
  expect(state).toEqual(before);
});

it.each([undefined, 'invalid'])(
  'does not manufacture a missing start time %#',
  (activeRunStartedAt) => {
    const view = render(<ActiveThinking conversation={{ ...waiting(), activeRunStartedAt }} />);
    expect(screen.getByText(/start time is unavailable/)).toBeVisible();
    expect(view.container.querySelector('time')).toBeNull();
  },
);

it.each(['reply', 'reasoning', 'tool', 'plan'] as const)(
  'uses observed %s activity instead of the empty waiting presentation',
  (kind) => {
    const initial = waiting();
    const state =
      kind === 'reasoning'
        ? foldLiveEvent(
            initial,
            {
              type: 'REASONING_MESSAGE_CONTENT',
              threadId: 'agent',
              runId: '1',
              messageId: 'reason',
              delta: 'Checking the file',
            },
            'agent',
          )
        : foldAgentEvent(initial, {
            seq: 2,
            appendedAt: at,
            kind:
              kind === 'reply'
                ? { type: 'agent_message', content: 'Checking the file' }
                : kind === 'tool'
                  ? {
                      type: 'tool_call_started',
                      call_id: 'tool',
                      title: 'Read file',
                      status: 'running',
                    }
                  : { type: 'plan_set', entries: [{ title: 'Read file', status: 'in_progress' }] },
          });
    expect(hasVisibleTurnProgress(state)).toBe(true);
    render(<ActiveThinking conversation={state} />);
    expect(screen.queryByText('Waiting for a response')).toBeNull();
    expect(screen.getByText('Thinking')).toBeVisible();
    expect(screen.getByText(/Still waiting for this turn to finish/)).toBeVisible();
  },
);

it('does not borrow another turn activity or count an empty live fragment as progress', () => {
  const state = {
    ...waiting(),
    messages: [
      {
        id: 'old',
        runId: 'old',
        role: 'assistant' as const,
        content: 'Earlier answer',
        status: 'complete' as const,
      },
      {
        id: 'empty',
        runId: '1',
        role: 'assistant' as const,
        content: '',
        status: 'streaming' as const,
        liveOnly: true,
      },
    ],
  };
  expect(hasVisibleTurnProgress(state)).toBe(false);
});

it.each(['turn_failed', 'turn_completed', 'turn_interrupted'])(
  'removes waiting only after committed %s',
  (type) => {
    const state = foldAgentEvent(waiting(), {
      seq: 2,
      appendedAt: at,
      kind: { type, turn_sequence: 1, failure: { class: 'agent_turn_timeout', detail: '' } },
    });
    render(<ActiveThinking conversation={state} />);
    expect(screen.queryByText('Waiting for a response')).toBeNull();
    expect(screen.queryByText(/Still waiting for this turn to finish/)).toBeNull();
  },
);
