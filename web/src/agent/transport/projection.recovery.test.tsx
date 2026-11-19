import { render } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { V2AgentCursor, V2AgentEventItem } from '@cma/client';
import { conversationFromTranscript } from '../conversation';
import { mergeTranscriptSnapshot } from '../chronology';
import { MessageStream } from '../MessageStream';
import type { ConversationState } from '../model';
import { foldV2Frame, foldV2Item } from './projection';

const agent = 'synthetic-agent';
const incarnation = 'a'.repeat(64);
const at = '2026-11-01T00:00:00Z';
const empty = (): ConversationState => ({
  ...conversationFromTranscript({ agentId: agent, throughSeq: 0, turns: [] }),
  agentIncarnation: incarnation,
});
const event = (state: ConversationState, sequence: number, kind: V2AgentEventItem['kind']) =>
  foldV2Item(state, {
    sequence,
    cursor: `ag1.${incarnation}.${sequence}` as V2AgentCursor,
    appended_at: at,
    kind,
    response_binding:
      kind?.type === 'turn_started'
        ? {
            command_id: `cmd_${sequence}`,
            response_id: `resp_${sequence}`,
            turn_sequence: sequence,
          }
        : null,
  });
const start = () => event(empty(), 1, { type: 'turn_started', turn_sequence: 1 });
const gap = (
  state: ConversationState,
  turn_sequence: number | null = null,
  reason: 'lagged' | 'reconnected' = 'lagged',
) =>
  foldV2Frame(
    state,
    {
      type: 'cma.agent.live_gap',
      data: { agent_id: agent, incarnation, turn_sequence, reason, frames: 1 },
    },
    agent,
  );
const partial = (state: ConversationState) =>
  foldV2Frame(
    state,
    {
      type: 'cma.agent.live',
      data: {
        agent_id: agent,
        incarnation,
        turn_sequence: 1,
        after_sequence: 1,
        message_id: 'partial',
        kind: 'text_delta',
        delta: 'Visible partial reply',
        observed_at: at,
      },
    },
    agent,
  );
const terminals = ['turn_failed', 'turn_completed', 'turn_cancelled'] as const;
const terminal = (type: (typeof terminals)[number]): V2AgentEventItem['kind'] =>
  type === 'turn_failed'
    ? { type, turn_sequence: 1, failure: { class: 'runtime', detail: 'Synthetic failure' } }
    : { type, turn_sequence: 1 };

it('clears a failed turn refresh and never appends another on later idle gaps', () => {
  let state = gap(start(), 1);
  expect(state.recovery.filter((notice) => notice.clearsOn)).toHaveLength(1);
  state = event(state, 2, terminal('turn_failed'));
  expect(state.recovery.filter((notice) => notice.clearsOn)).toHaveLength(0);
  state = gap(state);
  state = event(state, 3, { type: 'status_changed', from: 'working', to: 'ready' });
  state = gap(gap(state), null, 'reconnected');
  expect(state.turnOutcomes?.['1']).toBe('failed');
  expect(state.recovery).toEqual([
    expect.objectContaining({ title: 'Agent run failed', runId: '1' }),
  ]);
  const view = render(<MessageStream conversation={state} onApproval={vi.fn()} />);
  expect(view.container.querySelector('.message-stream__live-recovery-slot')).toBeEmptyDOMElement();
  expect(view.queryByText('Refreshing live updates…')).not.toBeInTheDocument();
  expect(view.queryByText('Thinking')).not.toBeInTheDocument();
});

it.each(terminals)('retains partial output and finite diagnostics after %s', (type) => {
  let state = gap(partial(start()), 1);
  state = event(state, 2, { type: 'history_gap', frames: 1 });
  state = event(state, 3, terminal(type));
  for (let pass = 0; pass < 3; pass += 1) state = gap(gap(state), 1, 'reconnected');
  expect(state.messages[0]?.content).toBe('Visible partial reply');
  expect(state.recovery.filter((notice) => notice.clearsOn)).toHaveLength(0);
  expect(state.recovery.filter((notice) => notice.title === 'Incomplete live output')).toHaveLength(
    1,
  );
  expect(state.recovery).toContainEqual(
    expect.objectContaining({ title: 'Incomplete conversation history' }),
  );
  const view = render(<MessageStream conversation={state} onApproval={vi.fn()} />);
  expect(view.getByText('Visible partial reply')).toBeVisible();
  expect(view.getByText('Incomplete live output')).toBeVisible();
  expect(view.container.querySelector('[data-streaming]')).toBeNull();
  expect(view.container.querySelector('.message-stream__live-recovery-slot')).toBeEmptyDOMElement();
});

it.each(terminals)('keeps a newer turn and its notice when an older %s arrives', (type) => {
  let state = event(start(), 3, { type: 'turn_started', turn_sequence: 3 });
  state = gap(state, 3);
  state = event(state, 4, terminal(type));
  expect(state).toMatchObject({
    activeRunId: '3',
    activeRunStartedAt: at,
    latestTurnId: '3',
    responseBinding: { turnSequence: 3 },
  });
  expect(state.recovery.filter((notice) => notice.clearsOn)).toEqual([
    expect.objectContaining({ runId: '3', title: 'Refreshing live updates…' }),
  ]);
  expect(gap(state, 1)).toBe(state);
});

it.each(terminals)('reconciles terminal %s snapshots without losing partial output', (type) => {
  let state = gap(partial(start()), 1);
  const snapshot = event(start(), 2, terminal(type));
  state = mergeTranscriptSnapshot(state, snapshot);
  state = mergeTranscriptSnapshot(state, snapshot);
  state = gap(state);
  expect(state.activeRunId).toBeUndefined();
  expect(state.messages[0]?.content).toBe('Visible partial reply');
  expect(state.recovery.filter((notice) => notice.clearsOn)).toHaveLength(0);
  expect(state.recovery.filter((notice) => notice.title === 'Incomplete live output')).toHaveLength(
    1,
  );
  const reloaded = gap(snapshot);
  expect(reloaded.recovery.some((notice) => notice.clearsOn)).toBe(false);
  state = event(state, 3, { type: 'turn_started', turn_sequence: 3 });
  state = gap(state, 3);
  expect(state.recovery.filter((notice) => notice.clearsOn)).toEqual([
    expect.objectContaining({ runId: '3', title: 'Refreshing live updates…' }),
  ]);
});

it('deduplicates idle connection gaps without claiming a reply is being restored', () => {
  let state = gap(empty());
  state = event(state, 1, { type: 'status_changed', from: 'paused', to: 'ready' });
  state = gap(state);
  expect(state.recovery).toEqual([expect.objectContaining({ title: 'Live updates interrupted' })]);
  state = event(state, 2, { type: 'turn_started', turn_sequence: 2 });
  expect(state.recovery).toEqual([]);
});
