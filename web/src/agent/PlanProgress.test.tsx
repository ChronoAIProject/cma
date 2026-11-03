import { fireEvent, render, screen, within } from '@testing-library/react';
import { expect, it } from 'vitest';
import { conversationFromTranscript, foldAgentEvent, foldLiveEvent } from './conversation';
import { MessageStream } from './MessageStream';
import { mergeTranscriptSnapshot } from './chronology';

const steps = [
  { title: 'Read the instructions', status: 'completed' },
  { title: 'Implement the change', status: 'in_progress' },
  { title: 'Verify the result', status: 'pending' },
];
const started = () =>
  foldAgentEvent(conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] }), {
    seq: 1,
    appendedAt: '2026-11-03T00:00:00Z',
    kind: { type: 'turn_started', turn_sequence: 1 },
  });

it('replays one current plan with completed count and current step after reconnect', () => {
  let conversation = foldLiveEvent(
    started(),
    {
      type: 'STATE_DELTA',
      threadId: 'agent',
      runId: '1',
      delta: [{ op: 'replace', path: '/plan', value: steps }],
    },
    'agent',
  );
  const report = {
    seq: 2,
    appendedAt: '2026-11-03T00:00:01Z',
    kind: { type: 'plan_set', entries: steps },
  };
  conversation = foldAgentEvent(conversation, report);
  const view = render(<MessageStream conversation={conversation} onApproval={() => {}} />);
  conversation = foldAgentEvent(conversation, report);
  view.rerender(<MessageStream conversation={conversation} onApproval={() => {}} />);
  const cards = screen.getAllByRole('region', { name: 'Plan' });
  expect(cards).toHaveLength(1);
  expect(within(cards[0]!).getByText('Completed 1/3 steps')).toBeVisible();
  expect(within(cards[0]!).getByText('Current step 2: Implement the change')).toBeVisible();
  const progress = within(cards[0]!).getByRole('progressbar', { name: 'Reported plan progress' });
  expect(progress).toHaveAttribute('aria-valuenow', '1');
  expect(progress).toHaveAttribute('aria-valuemax', '3');
});

it('replaces reordered and ambiguous plans without guessing progress, then clears the card', () => {
  let conversation = started();
  let sequence = 1;
  const report = (entries: unknown) => {
    conversation = foldAgentEvent(conversation, {
      seq: ++sequence,
      appendedAt: '2026-11-03T00:00:01Z',
      kind: { type: 'plan_set', entries },
    });
    return <MessageStream conversation={conversation} onApproval={() => {}} />;
  };
  const view = render(report([steps[2], steps[0], { title: 'Await review', status: 'blocked' }]));
  expect(screen.getByText('Progress unknown')).toBeVisible();
  expect(screen.queryByRole('progressbar')).not.toBeInTheDocument();
  expect(screen.getByText('blocked')).toBeVisible();
  expect(screen.queryByText(/Current step/)).not.toBeInTheDocument();
  view.rerender(report([steps[1], { title: 'Parallel check', status: 'in_progress' }, steps[0]]));
  expect(screen.getAllByRole('region', { name: 'Plan' })).toHaveLength(1);
  expect(screen.getByText('Current step 1: Implement the change')).toBeVisible();
  expect(screen.getByText('Multiple steps in progress')).toBeVisible();
  expect(screen.queryByRole('progressbar')).not.toBeInTheDocument();
  view.rerender(report([]));
  expect(screen.queryByRole('region', { name: 'Plan' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: /^Thinking/ })).toBeVisible();
});

it.each([
  ['turn_completed', 'Response finished · Plan completion not fully reported'],
  ['turn_failed', 'Response failed'],
  ['turn_interrupted', 'Response cancelled'],
])('keeps the reported plan states when %s ends the response', (type, summary) => {
  let conversation = foldAgentEvent(started(), {
    seq: 2,
    appendedAt: '2026-11-03T00:00:01Z',
    kind: { type: 'plan_set', entries: steps },
  });
  const view = render(<MessageStream conversation={conversation} onApproval={() => {}} />);
  conversation = foldAgentEvent(conversation, {
    seq: 3,
    appendedAt: '2026-11-03T00:00:02Z',
    kind: { type, turn_sequence: 1 },
  });
  view.rerender(<MessageStream conversation={conversation} onApproval={() => {}} />);
  expect(screen.getByText(summary)).toBeVisible();
  const disclosure = screen.getByRole('button', { name: 'View plan and operations' });
  expect(disclosure).toHaveAttribute('aria-expanded', 'false');
  fireEvent.click(disclosure);
  const plan = screen.getByRole('region', { name: 'Plan' });
  expect(within(plan).getByText('Completed 1/3 steps')).toBeVisible();
  expect(within(plan).getByText('Waiting')).toBeVisible();
  expect(within(plan).queryByText(/Current step/)).not.toBeInTheDocument();
  expect(screen.queryByText(/Completed all/)).not.toBeInTheDocument();
});

it('retains a successful fully reported plan across live completion and transcript recovery', () => {
  let conversation = foldAgentEvent(started(), {
    seq: 2,
    appendedAt: '2026-11-03T00:00:01Z',
    kind: { type: 'plan_set', entries: steps.map((step) => ({ ...step, status: 'completed' })) },
  });
  conversation = foldLiveEvent(
    conversation,
    {
      type: 'RUN_FINISHED',
      threadId: 'agent',
      runId: '1',
      outcome: { type: 'complete' },
    },
    'agent',
  );
  const view = render(<MessageStream conversation={conversation} onApproval={() => {}} />);
  expect(screen.getByText('Completed all 3 steps')).toBeVisible();
  conversation = mergeTranscriptSnapshot(
    conversation,
    conversationFromTranscript({
      agentId: 'agent',
      throughSeq: 3,
      turns: [
        {
          turnSequence: 1,
          startedAt: '2026-11-03T00:00:00Z',
          messages: [],
          ownerMessage: 'Plan the task',
          agentMessages: [],
          outcome: { type: 'completed', at: '2026-11-03T00:00:02Z' },
        },
      ],
    }),
  );
  view.rerender(<MessageStream conversation={conversation} onApproval={() => {}} />);
  expect(screen.getByText('Completed all 3 steps')).toBeVisible();
  expect(screen.getByRole('button', { name: 'View plan and operations' })).toHaveAttribute(
    'aria-expanded',
    'false',
  );
});

it('keeps approvals visible and the plan current while the public stream awaits a decision', () => {
  let conversation = foldAgentEvent(started(), {
    seq: 2,
    appendedAt: '2026-11-03T00:00:01Z',
    kind: { type: 'plan_set', entries: steps },
  });
  conversation = foldLiveEvent(
    conversation,
    {
      type: 'RUN_FINISHED',
      threadId: 'agent',
      runId: '1',
      outcome: { type: 'interrupt', interrupts: [{ id: 'permission', reason: 'Apply patch' }] },
    },
    'agent',
  );
  render(<MessageStream conversation={conversation} onApproval={() => {}} />);
  expect(screen.getByRole('button', { name: 'Allow' })).toBeVisible();
  expect(screen.getByText('Current step 2: Implement the change')).toBeVisible();
  expect(screen.queryByText('Outcome unknown')).not.toBeInTheDocument();
  expect(screen.queryByText('Response cancelled')).not.toBeInTheDocument();
});

it('folds operations by default while keeping failures and approvals discoverable without a plan', () => {
  let conversation = foldAgentEvent(started(), {
    seq: 2,
    appendedAt: '2026-11-03T00:00:01Z',
    kind: {
      type: 'tool_call_started',
      call_id: 'read',
      title: 'Read instructions',
      status: 'completed',
    },
  });
  const view = render(<MessageStream conversation={conversation} onApproval={() => {}} />);
  const operation = screen.getByRole('button', { name: '1 tool call' });
  expect(operation).toHaveAttribute('aria-expanded', 'false');
  expect(
    screen.queryByRole('article', { name: 'Tool call: Read instructions' }),
  ).not.toBeInTheDocument();
  expect(screen.queryByRole('progressbar')).not.toBeInTheDocument();
  expect(screen.queryByText(/remaining|ETA/i)).not.toBeInTheDocument();
  conversation = foldAgentEvent(conversation, {
    seq: 3,
    appendedAt: '2026-11-03T00:00:02Z',
    kind: { type: 'tool_call_updated', call_id: 'read', status: 'failed' },
  });
  conversation = foldAgentEvent(conversation, {
    seq: 4,
    appendedAt: '2026-11-03T00:00:03Z',
    kind: { type: 'approval_requested', approval_id: 'permission', action: 'Read another file' },
  });
  view.rerender(<MessageStream conversation={conversation} onApproval={() => {}} />);
  expect(screen.getByRole('article', { name: 'Tool call: Read instructions' })).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: '1 tool call · 1 failed' }));
  expect(screen.getByRole('button', { name: '1 tool call · 1 failed' })).toBeVisible();
  expect(screen.getByRole('button', { name: 'Allow' })).toBeVisible();
});

it.each([
  ['provider_failure', 'Response failed'],
  ['owner', 'Response cancelled'],
])('projects public stream error %s into the same plan outcome', (code, summary) => {
  let conversation = foldAgentEvent(started(), {
    seq: 2,
    appendedAt: '2026-11-03T00:00:01Z',
    kind: { type: 'plan_set', entries: steps },
  });
  conversation = foldLiveEvent(
    conversation,
    {
      type: 'RUN_ERROR',
      threadId: 'agent',
      runId: '1',
      code,
      message: 'The response stopped.',
    },
    'agent',
  );
  render(<MessageStream conversation={conversation} onApproval={() => {}} />);
  expect(screen.getByText(summary)).toBeVisible();
  expect(screen.queryByText(/Completed all|Current step/)).not.toBeInTheDocument();
});

it('does not regress a cleared plan when an older public replay frame arrives', () => {
  const report = {
    type: 'STATE_DELTA',
    threadId: 'agent',
    runId: '1',
    cursor: 2,
    delta: [{ op: 'replace', path: '/plan', value: steps }],
  };
  let conversation = foldLiveEvent(started(), report, 'agent');
  conversation = foldLiveEvent(
    conversation,
    {
      ...report,
      cursor: 3,
      delta: [{ op: 'replace', path: '/plan', value: [] }],
    },
    'agent',
  );
  conversation = foldLiveEvent(conversation, report, 'agent');
  render(<MessageStream conversation={conversation} onApproval={() => {}} />);
  expect(screen.queryByRole('region', { name: 'Plan' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: /^Thinking/ })).toBeVisible();
});

it('keeps operation disclosures in their own turns', () => {
  let conversation = started();
  const reports = [
    {
      type: 'tool_call_started',
      call_id: 'first',
      title: 'First turn operation',
      status: 'completed',
    },
    { type: 'turn_completed', turn_sequence: 1 },
    { type: 'turn_started', turn_sequence: 4 },
    {
      type: 'tool_call_started',
      call_id: 'second',
      title: 'Second turn operation',
      status: 'completed',
    },
  ];
  reports.forEach((kind, index) => {
    conversation = foldAgentEvent(conversation, {
      seq: index + 2,
      appendedAt: '2026-11-03T00:00:02Z',
      kind,
    });
  });
  render(<MessageStream conversation={conversation} onApproval={() => {}} />);
  const disclosures = screen.getAllByRole('button', { name: '1 tool call' });
  expect(disclosures).toHaveLength(2);
  fireEvent.click(disclosures[0]!);
  expect(screen.getByRole('article', { name: 'Tool call: First turn operation' })).toBeVisible();
  expect(
    screen.queryByRole('article', { name: 'Tool call: Second turn operation' }),
  ).not.toBeInTheDocument();
});

it('does not turn an incomplete plan report into a fully completed plan', () => {
  let conversation = foldAgentEvent(started(), {
    seq: 2,
    appendedAt: '2026-11-03T00:00:01Z',
    kind: { type: 'plan_set', entries: [steps[0], { title: 'Missing status' }] },
  });
  conversation = foldAgentEvent(conversation, {
    seq: 3,
    appendedAt: '2026-11-03T00:00:02Z',
    kind: { type: 'turn_completed', turn_sequence: 1 },
  });
  render(<MessageStream conversation={conversation} onApproval={() => {}} />);
  expect(screen.getByText('Progress unknown')).toBeVisible();
  expect(screen.getByText('Some plan steps unavailable')).toBeVisible();
  expect(screen.queryByRole('progressbar')).not.toBeInTheDocument();
  expect(screen.queryByText(/Completed all/)).not.toBeInTheDocument();
});
