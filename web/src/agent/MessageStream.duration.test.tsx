import { render, screen, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { V2AgentCursor } from '@chronoai/cma-sdk';
import type { AgentEvent, AgentLiveEvent } from '../api/types';
import { conversationFromTranscript, foldAgentEvent, foldLiveEvent } from './conversation';
import { mergeTranscriptSnapshot } from './chronology';
import { MessageStream } from './MessageStream';
import { foldV2Item } from './transport/projection';
import { workedFor } from './turnTiming';

const start = '2026-10-28T00:00:00Z';
const end = '2026-10-28T02:03:04Z';
const empty = () => conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] });
const events: AgentEvent[] = [
  { seq: 1, appendedAt: start, kind: { type: 'turn_started', turn_sequence: 1 } },
  { seq: 2, appendedAt: start, kind: { type: 'agent_message', content: 'Checking.' } },
  { seq: 3, appendedAt: end, kind: { type: 'agent_message', content: 'Finished.' } },
  { seq: 4, appendedAt: end, kind: { type: 'turn_completed', turn_sequence: 1 } },
];
const draw = (conversation: ReturnType<typeof empty>) => (
  <MessageStream conversation={conversation} onApproval={vi.fn()} />
);

it('shows elapsed time immediately on completion, only under the final reply, and after replay', () => {
  const active = events.slice(0, -1).reduce(foldAgentEvent, empty());
  const view = render(draw(active));
  expect(screen.queryByLabelText('Turn duration')).not.toBeInTheDocument();
  view.rerender(draw(foldAgentEvent(active, events[3]!)));
  expect(screen.getByLabelText('Turn duration')).toHaveTextContent(
    'Worked for 2 hours 3 minutes 4 seconds.',
  );
  expect(
    within(screen.getByText('Checking.').closest('article')!).queryByLabelText('Turn duration'),
  ).toBeNull();
  expect(
    within(screen.getByText('Finished.').closest('article')!).getByLabelText('Turn duration'),
  ).toBeVisible();
  view.unmount();
  render(draw(events.reduce(foldAgentEvent, empty())));
  expect(screen.getByLabelText('Turn duration')).toHaveTextContent(
    'Worked for 2 hours 3 minutes 4 seconds.',
  );
});

it('keeps the original start through AG-UI approval pauses and reconnects', () => {
  const live = (type: string, timestamp: string, extra = {}): AgentLiveEvent => ({
    type,
    threadId: 'agent',
    runId: '1',
    timestamp,
    ...extra,
  });
  let state = foldLiveEvent(empty(), live('RUN_STARTED', start), 'agent');
  state = foldLiveEvent(
    state,
    live('RUN_FINISHED', '2026-10-28T01:00:00Z', {
      outcome: { interrupts: [{ id: 'approval' }] },
    }),
    'agent',
  );
  expect(workedFor(state.turnTiming?.['1'])).toBeUndefined();
  state = foldLiveEvent(state, live('RUN_STARTED', '2026-10-28T01:30:00Z'), 'agent');
  expect(state.activeRunStartedAt).toBe(start);
  state = foldLiveEvent(state, live('RUN_FINISHED', end), 'agent');
  expect(workedFor(state.turnTiming?.['1'])).toBe('Worked for 2 hours 3 minutes 4 seconds.');
  state = foldLiveEvent(state, live('RUN_FINISHED', '2026-10-28T03:00:00Z'), 'agent');
  expect(state.turnTiming?.['1']?.finishedAt).toBe(end);
});

it('recovers durations through the Responses v2 public event projection', () => {
  let state = foldV2Item(empty(), {
    cursor: `ag1.${'a'.repeat(64)}.1` as V2AgentCursor,
    sequence: 1,
    appended_at: start,
    kind: { type: 'turn_started', turn_sequence: 1 },
    response_binding: null,
  });
  state = foldV2Item(state, {
    cursor: `ag1.${'a'.repeat(64)}.4` as V2AgentCursor,
    sequence: 4,
    appended_at: end,
    kind: { type: 'turn_completed', turn_sequence: 1 },
    response_binding: null,
  });
  expect(workedFor(state.turnTiming?.['1'])).toBe('Worked for 2 hours 3 minutes 4 seconds.');
});

it('preserves completed timing when transcript hydration has no terminal timestamp', () => {
  const complete = events.reduce(foldAgentEvent, empty());
  const snapshot = conversationFromTranscript({
    agentId: 'agent',
    throughSeq: 4,
    turns: [
      {
        turnSequence: 1,
        startedAt: start,
        ownerMessage: '',
        agentMessages: ['Finished.'],
        messages: [{ sequence: 3, role: 'assistant', content: 'Finished.', appendedAt: end }],
        outcome: { type: 'completed' },
      },
    ],
  });
  expect(workedFor(mergeTranscriptSnapshot(complete, snapshot, 4, true).turnTiming?.['1'])).toBe(
    'Worked for 2 hours 3 minutes 4 seconds.',
  );
  expect(workedFor(mergeTranscriptSnapshot(snapshot, complete, 4, true).turnTiming?.['1'])).toBe(
    'Worked for 2 hours 3 minutes 4 seconds.',
  );
});

it.each([
  [undefined, undefined],
  [{ startedAt: start }, undefined],
  [{ startedAt: 'invalid', finishedAt: end }, undefined],
  [{ startedAt: end, finishedAt: start }, undefined],
  [{ startedAt: start, finishedAt: start }, 'Worked for 0 seconds.'],
  [{ startedAt: start, finishedAt: '2026-10-28T00:01:01Z' }, 'Worked for 1 minute 1 second.'],
  [
    { startedAt: start, finishedAt: '2026-10-29T01:00:00Z' },
    'Worked for 25 hours 0 minutes 0 seconds.',
  ],
] as const)(
  'formats recorded elapsed time without inventing unknown durations: %j',
  (timing, expected) => {
    expect(workedFor(timing)).toBe(expected);
  },
);
