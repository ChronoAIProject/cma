import { render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import fixture from '../../test/externalApprovalProjection.fixture.json';
import {
  externalApprovalProjection,
  externalApprovalKind,
} from '../../api-client/agent-events-v2/external-approval';
import {
  v2AgentEventPage,
  v2AgentEvent,
  v2AgentLiveGap,
} from '../../api-client/agent-events-v2/types';
import { conversationFromTranscript, foldAgentEvent, foldLiveEvent } from '../conversation';
import type { ConversationState } from '../model';
import { MessageStream } from '../MessageStream';
import { foldV2Item, foldV2Frame } from './projection';
import { foldExternalApproval, withApprovalProjection } from './externalApprovalProjection';

const agent = fixture.waiting_snapshot.agent_id;
const projection = externalApprovalProjection(new Headers(fixture.response_headers), true);
function waiting() {
  const page = v2AgentEventPage(fixture.waiting_snapshot, agent, 100, undefined, true);
  return page.items.reduce<ConversationState>(
    foldV2Item,
    withApprovalProjection(
      {
        ...conversationFromTranscript({ agentId: agent, throughSeq: 0, turns: [] }),
        agentIncarnation: page.incarnation,
      },
      projection,
    ),
  );
}
const request = () => externalApprovalKind(fixture.live_requested.data.kind);
const resolution = () => externalApprovalKind(fixture.live_resolved.data.kind);
const frame = (value: typeof fixture.live_resolved | typeof fixture.live_terminal) => ({
  type: 'cma.agent.event' as const,
  data: v2AgentEvent(value.data, true),
});

it('renders a decoded exactly bound wait and never offers native approval controls', () => {
  const state = waiting();
  const approve = vi.fn();
  render(<MessageStream conversation={state} onApproval={approve} />);
  expect(screen.getByRole('heading', { name: 'Waiting for your approval' })).toBeVisible();
  expect(screen.getByRole('link', { name: 'Review and approve' })).toHaveAttribute(
    'href',
    fixture.live_requested.data.kind.approve_url,
  );
  expect(screen.queryByText('Thinking')).toBeNull();
  expect(screen.queryByRole('button', { name: 'Allow' })).toBeNull();
  expect(screen.getByText('Synthetic retained partial output.')).toBeVisible();
  expect(state.responseBinding).toEqual({
    commandId: 'cmd_approval_fixture',
    responseId: 'resp_approval_fixture',
    turnSequence: 3,
  });
  expect(approve).not.toHaveBeenCalled();
});

it('settles approval without ending the still-active turn', () => {
  const state = foldV2Frame(waiting(), frame(fixture.live_resolved), agent);
  expect(state.activeRunId).toBe('3');
  expect(state.turnOutcomes?.['3']).toBeUndefined();
  render(<MessageStream conversation={state} onApproval={vi.fn()} />);
  expect(screen.queryByRole('link', { name: 'Review and approve' })).toBeNull();
  expect(screen.getByText('Thinking')).toBeVisible();
});

it('terminal history wins over later receipts and gaps and preserves partial output', () => {
  const ended = foldV2Frame(waiting(), frame(fixture.live_terminal), agent);
  expect(foldExternalApproval(ended, { ...request(), revision: 9 }, 11)).toBe(ended);
  expect(foldExternalApproval(ended, resolution(), 11)).toBe(ended);
  expect(
    foldV2Frame(
      ended,
      { type: 'cma.agent.live_gap', data: v2AgentLiveGap(fixture.late_gap.data) },
      agent,
    ),
  ).toBe(ended);
  render(<MessageStream conversation={ended} onApproval={vi.fn()} />);
  expect(screen.getByText('The approval wait timed out. This turn has ended.')).toBeVisible();
  expect(screen.getByText('Synthetic retained partial output.')).toBeVisible();
  expect(screen.queryByRole('link', { name: 'Review and approve' })).toBeNull();
  expect(screen.queryByText('Waiting for your approval')).toBeNull();
});

it('replays identical revisions without resetting the deadline and rejects revision conflicts', () => {
  const state = waiting();
  expect(foldExternalApproval(state, request(), 10)).toBe(state);
  const shorter = externalApprovalKind({
    ...request(),
    revision: 2,
    deadline_at: '2026-01-01T00:04:00.000Z',
  });
  const updated = foldExternalApproval(state, shorter, 10);
  expect(foldExternalApproval(updated, request(), 11)).toBe(updated);
  expect(() => foldExternalApproval(updated, { ...request(), revision: 2 }, 11)).toThrow();
  expect(() => foldExternalApproval(updated, { ...request(), revision: 3 }, 11)).toThrow('budget');
  expect(() => foldExternalApproval(state, { ...resolution(), revision: 1 }, 10)).toThrow(
    'conflicting',
  );
});

it('never reopens an approved receipt or changes another operation', () => {
  const state = waiting();
  const second = foldExternalApproval(state, { ...request(), approval_id: 'ea_second' }, 10);
  const resolved = foldExternalApproval(second, resolution(), 11);
  expect(resolved.approvals).toHaveLength(2);
  expect(resolved.approvals.filter((item) => item.decision === undefined)).toHaveLength(1);
  expect(foldExternalApproval(resolved, { ...request(), revision: 3 }, 12)).toBe(resolved);
  expect(() =>
    foldExternalApproval(
      resolved,
      externalApprovalKind({ ...resolution(), revision: 3, resolution: 'denied' }),
      12,
    ),
  ).toThrow('conflicts');
});

it('refuses missing binding/support and isolates later active work', () => {
  const state = waiting();
  const newRequest = { ...request(), approval_id: 'ea_new' };
  expect(() =>
    foldExternalApproval({ ...state, responseBinding: undefined }, newRequest, 10),
  ).toThrow('binding');
  expect(() =>
    foldExternalApproval({ ...state, externalApprovalProjection: undefined }, newRequest, 10),
  ).toThrow('negotiated');
  const later = foldAgentEvent(state, {
    seq: 10,
    appendedAt: '2026-01-01T00:06:00.000Z',
    kind: { type: 'turn_started', turn_sequence: 10 },
  });
  expect(foldExternalApproval(later, newRequest, 11)).toBe(later);
  const resolved = foldExternalApproval(later, resolution(), 11);
  expect(resolved.activeRunId).toBe('10');
});

it('keeps read-only and untrusted destinations non-actionable without converting the wait to native approval', () => {
  const state = withApprovalProjection(waiting(), { supported: true, canReview: false });
  render(<MessageStream conversation={state} onApproval={vi.fn()} />);
  expect(screen.getByText('Waiting for your approval')).toBeVisible();
  expect(screen.queryByRole('link')).toBeNull();
  expect(screen.queryByRole('button', { name: 'Allow' })).toBeNull();
});

it('native observations cannot resolve or replace external approval records', () => {
  const state = waiting();
  const id = state.approvals[0]!.id;
  expect(() =>
    foldAgentEvent(state, {
      seq: 10,
      appendedAt: '',
      kind: { type: 'approval_requested', approval_id: id },
    }),
  ).toThrow('conflicts');
  const resolved = foldAgentEvent(state, {
    seq: 10,
    appendedAt: '',
    kind: { type: 'approval_resolved', approval_id: id, decision: 'allow' },
  });
  expect(resolved.approvals).toBe(state.approvals);
  const interrupted = foldLiveEvent(
    state,
    {
      type: 'RUN_FINISHED',
      threadId: agent,
      runId: '3',
      outcome: { type: 'interrupt', interrupts: [{ id, reason: 'Native collision' }] },
    },
    agent,
  );
  expect(interrupted.approvals).toBe(state.approvals);
});
