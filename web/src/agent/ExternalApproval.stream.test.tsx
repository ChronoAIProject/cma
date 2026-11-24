import { render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { MessageStream } from './MessageStream';
import { conversationFromTranscript, foldAgentEvent } from './conversation';
import {
  externalApprovalFixture as approval,
  externalReviewPolicyFixture as policy,
} from './externalApproval.fixture';
import { currentExternalApproval } from './externalApproval';

const at = approval.waitingSince;
function waiting() {
  return {
    ...conversationFromTranscript({
      agentId: 'agent-fixture',
      throughSeq: 3,
      turns: [
        {
          turnSequence: 1,
          startedAt: at,
          ownerMessage: 'Check the draft',
          agentMessages: ['Partial answer'],
          messages: [
            { sequence: 2, role: 'user' as const, content: 'Check the draft', appendedAt: at },
            { sequence: 3, role: 'assistant' as const, content: 'Partial answer', appendedAt: at },
          ],
          outcome: { type: 'active' },
        },
      ],
    }),
    approvals: [approval],
    externalApprovalReviewPolicy: policy,
  };
}

it('prioritizes the current approval over Thinking/empty Activity without native command controls', () => {
  const onApproval = vi.fn();
  render(<MessageStream conversation={waiting()} onApproval={onApproval} />);
  expect(screen.getByRole('heading', { name: 'Waiting for your approval' })).toBeVisible();
  expect(screen.getByRole('link', { name: 'Review and approve' })).toBeVisible();
  expect(screen.queryByRole('region', { name: 'What the agent is working through' })).toBeNull();
  for (const name of ['Allow', 'Deny', 'Abort'])
    expect(screen.queryByRole('button', { name })).toBeNull();
  expect(onApproval).not.toHaveBeenCalled();
});

it.each(['turn_failed', 'turn_completed', 'turn_interrupted'] as const)(
  'removes the review action on exact %s and preserves partial output',
  (type) => {
    const state = foldAgentEvent(waiting(), {
      seq: 4,
      appendedAt: at,
      kind: {
        type,
        turn_sequence: 1,
        ...(type === 'turn_failed' ? { failure: { class: 'approval_timeout', detail: '' } } : {}),
      },
    });
    const view = render(<MessageStream conversation={state} onApproval={vi.fn()} />);
    expect(screen.getByText('Partial answer')).toBeVisible();
    expect(screen.queryByRole('link', { name: 'Review and approve' })).toBeNull();
    expect(screen.queryByRole('heading', { name: 'Waiting for your approval' })).toBeNull();
    expect(view.container.querySelector('.approval .character-avatar')).toBeNull();
    expect(view.container.querySelector('.thinking')).toBeNull();
  },
);

it('does not let another turn approval suppress the current turn or become actionable', () => {
  const state = { ...waiting(), activeRunId: '5', latestTurnId: '5' };
  render(<MessageStream conversation={state} onApproval={vi.fn()} />);
  expect(currentExternalApproval(state)).toBeUndefined();
  expect(screen.queryByRole('link')).toBeNull();
  expect(screen.getByRole('region', { name: 'What the agent is working through' })).toBeVisible();
});

it('keeps at most one secondary live recovery notice during the approval wait', () => {
  const state = {
    ...waiting(),
    recovery: ['first', 'second'].map((id) => ({
      id,
      title: 'Refreshing live updates…',
      detail: 'Reconnect',
      runId: '1',
      clearsOn: 'live-update' as const,
    })),
  };
  render(<MessageStream conversation={state} onApproval={vi.fn()} />);
  expect(screen.getAllByText('Refreshing live updates…')).toHaveLength(1);
  expect(screen.getByRole('heading', { name: 'Waiting for your approval' })).toBeVisible();
});

it('does not infer an external wait from an active turn or empty activity', () => {
  const state = { ...waiting(), approvals: [] };
  render(<MessageStream conversation={state} onApproval={vi.fn()} />);
  expect(screen.queryByText('Waiting for your approval')).toBeNull();
  expect(screen.getByRole('region', { name: 'What the agent is working through' })).toBeVisible();
});
