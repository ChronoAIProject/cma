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
    response_binding: null,
  });
const gap = (state: ConversationState, turn: number | null = null) =>
  foldV2Frame(
    state,
    {
      type: 'cma.agent.live_gap',
      data: { agent_id: agent, incarnation, turn_sequence: turn, reason: 'lagged', frames: 1 },
    },
    agent,
  );

it.each(['stopped', 'deleted'] as const)(
  '%s settles only live recovery while preserving retained failures, history and partial text',
  (type) => {
    let state = event(empty(), 1, { type: 'turn_started', turn_sequence: 1 });
    state = event(state, 2, {
      type: 'turn_failed',
      turn_sequence: 1,
      failure: { class: 'runtime', detail: 'Retained synthetic failure' },
    });
    state = event(state, 3, { type: 'turn_started', turn_sequence: 3 });
    state = foldV2Frame(
      state,
      {
        type: 'cma.agent.live',
        data: {
          agent_id: agent,
          incarnation,
          turn_sequence: 3,
          after_sequence: 3,
          message_id: 'partial',
          kind: 'text_delta',
          delta: 'Retained partial text',
          observed_at: at,
        },
      },
      agent,
    );
    state = event(state, 4, { type: 'history_gap', frames: 1 });
    state = gap(state, 3);
    expect(state.recovery.filter((notice) => notice.clearsOn)).toHaveLength(1);
    const ended = event(state, 5, type === 'stopped' ? { type, reason: 'requested' } : { type });
    expect(ended.activeRunId).toBeUndefined();
    expect(ended.recovery.filter((notice) => notice.clearsOn)).toHaveLength(0);
    expect(ended.recovery).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          title: 'Agent run failed',
          runId: '1',
          detail:
            'The Agent run failed. Its delivery or task effects may be incomplete. Review the conversation before sending another message.',
        }),
        expect.objectContaining({ title: 'Incomplete conversation history' }),
        expect.objectContaining({ title: 'Incomplete live output', runId: '3' }),
      ]),
    );
    expect(ended.turnOutcomes?.['3']).toBeUndefined();
    let replayed = mergeTranscriptSnapshot(ended, state);
    replayed = mergeTranscriptSnapshot(replayed, ended);
    replayed = gap(gap(replayed, 3));
    expect(replayed.recovery.filter((notice) => notice.clearsOn)).toHaveLength(0);
    expect(
      replayed.recovery.filter((notice) => notice.title === 'Incomplete live output'),
    ).toHaveLength(1);
    const view = render(<MessageStream conversation={replayed} onApproval={vi.fn()} />);
    expect(view.getByText('Retained partial text')).toBeVisible();
    expect(
      view.getByText(
        'The Agent run failed. Its delivery or task effects may be incomplete. Review the conversation before sending another message.',
      ),
    ).toBeVisible();
    expect(document.body.innerHTML).not.toContain('Retained synthetic failure');
    expect(view.queryByText('Refreshing live updates…')).not.toBeInTheDocument();
    expect(view.container.querySelector('[data-streaming]')).toBeNull();
  },
);

it.each(['stopped', 'deleted'] as const)(
  '%s clears an idle connection notice without inventing a turn outcome',
  (type) => {
    const interrupted = gap(empty());
    expect(interrupted.recovery).toContainEqual(
      expect.objectContaining({ title: 'Live updates interrupted' }),
    );
    const ended = event(
      interrupted,
      1,
      type === 'stopped' ? { type, reason: 'requested' } : { type },
    );
    expect(ended.recovery).toEqual([]);
    expect(ended.turnOutcomes).toEqual(interrupted.turnOutcomes);
    expect(gap(ended).recovery).toEqual([]);
  },
);
