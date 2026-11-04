import { render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { conversationFromTranscript, foldAgentEvent } from './conversation';
import { MessageStream } from './MessageStream';
import { PlanCard, planStatus } from './PlanCard';

describe('planStatus', () => {
  it.each([
    ['completed', 'Done', 'neutral'],
    ['in_progress', 'In progress', 'active'],
    ['pending', 'Waiting', 'muted'],
    ['blocked', 'blocked', 'muted'],
  ])('reads %s as %s in the %s tone', (status, word, tone) => {
    expect(planStatus(status)).toEqual({ word, tone });
  });
});

describe('PlanCard', () => {
  it('lists every step with its title and status badge', () => {
    render(
      <PlanCard
        plan={{
          id: 'plan:1',
          entries: [
            { title: 'Read lemma 4.1 proof', status: 'completed' },
            { title: 'Draft bound for 4.2', status: 'in_progress' },
            { title: 'Run lean check', status: 'pending' },
          ],
        }}
      />,
    );
    const steps = within(screen.getByRole('region', { name: 'Plan' })).getAllByRole('listitem');
    expect(steps.map((step) => step.textContent)).toEqual([
      'Read lemma 4.1 proofDone',
      'Draft bound for 4.2In progress',
      'Run lean checkWaiting',
    ]);
    expect(steps[1]?.querySelector('.plan-card__icon[data-tone="active"]')).not.toBeNull();
    expect(steps[2]?.querySelector('.statedot')).toBeNull();
  });
});

describe('plans in the message stream', () => {
  const base = foldAgentEvent(
    conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] }),
    {
      seq: 1,
      appendedAt: '2026-10-21T00:00:00Z',
      kind: { type: 'turn_started', turn_sequence: 1 },
    },
  );
  const report = (seq: number, entries: unknown) =>
    foldAgentEvent(base, {
      seq,
      appendedAt: '2026-10-21T00:00:01Z',
      kind: { type: 'plan_set', entries },
    });

  it('renders a reported plan in the timeline', () => {
    render(
      <MessageStream
        conversation={report(2, [{ title: 'Run lean check', status: 'pending' }])}
        onApproval={vi.fn()}
      />,
    );
    expect(screen.getByRole('region', { name: 'Plan' })).toHaveTextContent('Run lean check');
  });

  it('shows no card for an emptied plan', () => {
    render(<MessageStream conversation={report(2, [])} onApproval={vi.fn()} />);
    expect(screen.queryByRole('region', { name: 'Plan' })).not.toBeInTheDocument();
  });
});
