import { act, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import fixture from '../../test/agentTurnTimeout.fixture.json';
import {
  v2AgentEventPage,
  v2AgentEvent,
  v2AgentLiveGap,
} from '../../api-client/agent-events-v2/types';
import { cursor } from '../../api-client/agent-events-v2/schema';
import type { ConversationState } from '../model';
import { agentFrames } from '../../api-client/agent-events-v2/stream';
import { conversationFromTranscript } from '../conversation';
import { foldV2Frame, foldV2Item } from './projection';
import { mergeTranscriptSnapshot } from '../chronology';
import { MessageStream } from '../MessageStream';
import { failureMessage } from '../failureMessage';
import { turnRetryInput } from '../turnRetryInput';

const agent = fixture.waiting_snapshot.agent_id;
function snapshot(value: unknown = fixture.waiting_snapshot) {
  const page = v2AgentEventPage(value, agent, 100);
  return page.items.reduce<ConversationState>(foldV2Item, {
    ...conversationFromTranscript({ agentId: agent, throughSeq: 0, turns: [] }),
    agentIncarnation: page.incarnation,
  });
}
const terminal = () => ({
  type: 'cma.agent.event' as const,
  data: v2AgentEvent(fixture.live_terminal.data),
});
const gap = () => ({
  type: 'cma.agent.live_gap' as const,
  data: v2AgentLiveGap(fixture.late_gap.data),
});
afterEach(() => vi.useRealTimers());

it('decodes the public fixture and keeps partial activity nonterminal regardless of browser time', () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-01-02T00:00:00Z'));
  const state = snapshot();
  const before = structuredClone(state);
  const view = render(<MessageStream conversation={state} onApproval={vi.fn()} />);
  expect(screen.getByText('Synthetic retained partial output.')).toBeVisible();
  expect(screen.getByText(/Still waiting for this turn to finish/)).toBeVisible();
  expect(view.container.querySelector('.thinking time')).toHaveAttribute(
    'datetime',
    fixture.waiting_snapshot.items[0]!.appended_at,
  );
  act(() => vi.advanceTimersByTime(86_400_000));
  view.unmount();
  const remount = render(<MessageStream conversation={snapshot()} onApproval={vi.fn()} />);
  expect(remount.container.querySelector('.thinking time')).toHaveAttribute(
    'datetime',
    fixture.waiting_snapshot.items[0]!.appended_at,
  );
  expect(screen.queryByText(/has ended|Waiting for your approval/)).toBeNull();
  expect(state).toEqual(before);
});

it('decodes actual SSE and snapshot terminals identically and retains original input and partial output', async () => {
  const waiting = snapshot();
  const frame = fixture.live_terminal;
  const response = new Response(
    `event: ${frame.type}\nid: ${frame.data.cursor}\ndata: ${JSON.stringify(frame.data)}\n\n`,
  );
  let live = waiting;
  for await (const decoded of agentFrames(
    response,
    new AbortController().signal,
    agent,
    cursor(fixture.waiting_snapshot.through_cursor),
  ))
    live = foldV2Frame(live, decoded, agent);
  const restored = snapshot(fixture.terminal_snapshot);
  expect(live).toEqual(restored);
  for (const state of [live, restored, mergeTranscriptSnapshot(waiting, restored)]) {
    expect(state.activeRunId).toBeUndefined();
    expect(state.turnOutcomes?.['1']).toBe('failed');
    expect(state.messages.map((message) => message.content)).toEqual([
      'Synthetic task input.',
      'Synthetic retained partial output.',
    ]);
    expect(turnRetryInput(state, '1')).toEqual({ input: 'Synthetic task input.' });
    expect(state.recovery).toEqual([
      expect.objectContaining({
        runId: '1',
        detail: 'This turn reached its time limit and has ended.',
        technicalDetails: { category: 'agent_turn_timeout' },
      }),
    ]);
    expect(foldV2Frame(state, gap(), agent)).toBe(state);
  }
});

it('does not replace the committed winner when its cursor is duplicated with conflicting content', () => {
  const ended = foldV2Frame(snapshot(), terminal(), agent);
  expect(foldV2Frame(ended, terminal(), agent)).toBe(ended);
  const conflicting = {
    ...terminal(),
    data: {
      ...terminal().data,
      kind: { type: 'turn_completed' as const, turn_sequence: 1 },
    },
  };
  expect(foldV2Frame(ended, conflicting, agent)).toBe(ended);
  render(<MessageStream conversation={ended} onApproval={vi.fn()} />);
  expect(screen.getAllByText('This turn reached its time limit and has ended.')).toHaveLength(1);
  expect(screen.queryByText(/Still waiting for this turn to finish/)).toBeNull();
});

it('preserves a newer active turn when the original turn terminal arrives late', () => {
  const started = foldV2Item(snapshot(), {
    ...terminal().data,
    kind: { type: 'turn_started', turn_sequence: 4 },
    response_binding: { command_id: 'cmd_new', response_id: 'resp_new', turn_sequence: 4 },
  });
  const data = terminal().data;
  const ended = foldV2Frame(
    started,
    {
      type: 'cma.agent.event',
      data: {
        ...data,
        sequence: 5,
        cursor: cursor(`ag1.${data.incarnation}.5`),
      },
    },
    agent,
  );
  expect(ended).toMatchObject({
    activeRunId: '4',
    latestTurnId: '4',
    responseBinding: {
      commandId: 'cmd_new',
      responseId: 'resp_new',
      turnSequence: 4,
    },
  });
  expect(ended.turnOutcomes?.['1']).toBe('failed');
  expect(turnRetryInput(ended, '1')).toBeUndefined();
  expect(foldV2Frame(ended, gap(), agent)).toBe(ended);
});

it.each(['agent_id', 'incarnation'] as const)('rejects a terminal with a different %s', (field) => {
  const frame = terminal();
  expect(() =>
    foldV2Frame(
      snapshot(),
      {
        ...frame,
        data: {
          ...frame.data,
          [field]: field === 'agent_id' ? 'agt_other' : 'b'.repeat(64),
        },
      },
      agent,
    ),
  ).toThrow('identity');
});

it('keeps schema, terminal binding and future-turn validation strict', () => {
  expect(() =>
    v2AgentEventPage({ ...fixture.terminal_snapshot, schema_version: 2 }, agent, 100),
  ).toThrow();
  for (const change of [
    { response_binding: fixture.waiting_snapshot.items[0]!.response_binding },
    {
      kind: {
        type: 'turn_failed',
        turn_sequence: 99,
        failure: { class: 'agent_turn_timeout', detail: '' },
      },
    },
  ])
    expect(() =>
      v2AgentEventPage(
        {
          ...fixture.terminal_snapshot,
          items: [
            ...fixture.waiting_snapshot.items,
            { ...fixture.terminal_snapshot.items[3], ...change },
          ],
        },
        agent,
        100,
      ),
    ).toThrow();
});

it.each(['upstream_timeout', 'agent_turn_no_progress', 'approval_timeout', 'runtime', undefined])(
  'does not classify %s as a generic server turn timeout',
  (category) => {
    expect(failureMessage(category).detail).not.toBe(
      'This turn reached its time limit and has ended.',
    );
  },
);
