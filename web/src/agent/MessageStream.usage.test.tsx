import { render, screen, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { AgentEvent } from '../api/types';
import { conversationFromTranscript, foldAgentEvent } from './conversation';
import { MessageStream } from './MessageStream';

const at = '2026-10-15T08:00:00Z';
const event = (seq: number, kind: AgentEvent['kind']): AgentEvent => ({
  seq,
  appendedAt: at,
  kind,
});
const events = [
  event(10, { type: 'turn_started', turn_sequence: 10 }),
  event(11, { type: 'owner_message', content: 'Inspect the workspace.' }),
  event(14, { type: 'agent_message', content: 'Checking the files.' }),
  event(17, {
    type: 'usage_updated',
    input_tokens: 13831,
    output_tokens: 61,
    context_used: 13892,
    turn_sequence: 10,
    turn_input_tokens: 13831,
    turn_output_tokens: 61,
  }),
  event(18, { type: 'agent_message', content: 'The workspace is ready.' }),
  event(19, { type: 'turn_completed', turn_sequence: 10 }),
  event(25, { type: 'turn_started', turn_sequence: 25 }),
  event(26, { type: 'owner_message', content: 'Confirm the next step.' }),
  event(29, {
    type: 'usage_updated',
    input_tokens: 27730,
    output_tokens: 75,
    context_used: 13913,
    turn_sequence: 25,
    turn_input_tokens: 13899,
    turn_output_tokens: 14,
  }),
  event(30, { type: 'agent_message', content: 'Ready for the next step.' }),
  event(31, { type: 'turn_completed', turn_sequence: 25 }),
];
const empty = () => conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] });

it('keeps each completed turn count under its final reply through a full history reload', () => {
  const state = events.reduce(foldAgentEvent, empty());
  const view = render(<MessageStream conversation={state} onApproval={vi.fn()} />);
  const verify = () => {
    const replies = screen.getAllByRole('article', { name: 'Chrono Agent' });
    expect(within(replies[0]!).queryByLabelText('Turn token usage')).not.toBeInTheDocument();
    expect(within(replies[1]!).getByLabelText('Turn input tokens: 13,831')).toHaveTextContent(
      '13,831 input',
    );
    expect(within(replies[1]!).getByLabelText('Turn output tokens: 61')).toHaveTextContent(
      '61 output',
    );
    expect(within(replies[2]!).getByLabelText('Turn input tokens: 13,899')).toHaveTextContent(
      '13,899 input',
    );
    expect(within(replies[2]!).getByLabelText('Turn output tokens: 14')).toHaveTextContent(
      '14 output',
    );
    expect(screen.getAllByLabelText('Turn token usage')).toHaveLength(2);
  };
  verify();
  view.unmount();
  render(
    <MessageStream conversation={events.reduce(foldAgentEvent, empty())} onApproval={vi.fn()} />,
  );
  verify();
});

it('does not attribute earlier usage to a new reply or display unknown usage as zero', () => {
  const started = events.slice(0, -1).reduce(foldAgentEvent, empty());
  const view = render(<MessageStream conversation={started} onApproval={vi.fn()} />);
  expect(screen.getAllByLabelText('Turn token usage')).toHaveLength(1);
  const noReport = [
    ...events.slice(0, 6),
    event(25, { type: 'turn_started', turn_sequence: 25 }),
    event(30, { type: 'agent_message', content: 'No usage report for this turn.' }),
    event(31, { type: 'turn_completed', turn_sequence: 25 }),
  ].reduce(foldAgentEvent, empty());
  view.rerender(<MessageStream conversation={noReport} onApproval={vi.fn()} />);
  expect(screen.getByText('Turn input unavailable · output unavailable')).toBeInTheDocument();
  expect(screen.getAllByLabelText('Turn input tokens: 13,831')).toHaveLength(1);
});
