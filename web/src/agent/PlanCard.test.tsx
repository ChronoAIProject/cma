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

it.each(['pending', 'in_progress'])(
  'asks to check reported %s steps after a turn ends without declaring failure',
  (status) => {
    render(
      <PlanCard
        active={false}
        outcome="completed"
        plan={{
          id: 'plan:1',
          runId: '1',
          entries: [
            { title: 'Write the answer', status: 'completed' },
            { title: 'Optional supporting file', status },
          ],
        }}
      />,
    );
    expect(screen.getByRole('status')).toHaveTextContent(
      'Turn ended · Check unfinished plan steps and output.',
    );
    expect(screen.getByRole('button', { name: 'View plan' })).toHaveAttribute(
      'aria-expanded',
      'false',
    );
    expect(screen.queryByText(/failed|verified success/i)).not.toBeInTheDocument();
  },
);

it.each([
  { entries: [{ title: 'Unknown step', status: 'blocked' }] },
  { entries: [{ title: 'Only surviving step', status: 'completed' }], incomplete: true },
])('asks to check unknown plan progress without inventing a result: %j', (report) => {
  render(<PlanCard active={false} outcome="completed" plan={{ id: 'plan:1', ...report }} />);
  expect(screen.getByRole('status')).toHaveTextContent(
    'Turn ended · Check plan progress and output.',
  );
  expect(screen.queryByRole('progressbar')).not.toBeInTheDocument();
});

it('identifies a fully reported plan as an Agent report rather than verified task acceptance', () => {
  render(
    <PlanCard
      active={false}
      outcome="completed"
      plan={{ id: 'plan:1', entries: [{ title: 'Write the answer', status: 'completed' }] }}
    />,
  );
  expect(screen.getByRole('status')).toHaveTextContent(
    'Turn ended · 1 plan step reported complete',
  );
  expect(screen.queryByText(/Check unfinished|Check plan progress/)).not.toBeInTheDocument();
});

it.each([
  { active: true, outcome: undefined, expected: 'Current step 1' },
  { active: false, outcome: undefined, expected: 'Outcome unknown' },
  { active: false, outcome: 'failed' as const, expected: 'Response failed' },
  { active: false, outcome: 'interrupted' as const, expected: 'Response cancelled' },
])('preserves the existing $expected presentation', ({ active, outcome, expected }) => {
  render(
    <PlanCard
      active={active}
      outcome={outcome}
      plan={{ id: 'plan:1', entries: [{ title: 'Continue work', status: 'in_progress' }] }}
    />,
  );
  expect(screen.getByRole('status')).toHaveTextContent(expected);
  expect(
    screen.queryByText(/Turn ended|Check unfinished|Check plan progress/),
  ).not.toBeInTheDocument();
});
