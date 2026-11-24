import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { ChannelReceiptFacts } from './ChannelReceiptFacts';
import { ChannelActivityView } from './ChannelActivity';
import type { ActivityConsumer, ActivityDelivery, ActivityPage } from './useChannelActivity';

const consumer: ActivityConsumer = {
  consumer_id: 'reviewer',
  display_name: 'Sample task',
  state: 'queued',
  dispatched_at: null,
  accepted_at: null,
  produced_at: null,
  completed_at: null,
  response_id: null,
  deliveries: [],
};
const delivery = (outcome: ActivityDelivery['outcome']): ActivityDelivery => ({
  step: 'answer',
  destination: 'origin',
  outcome,
  observed_at: '2026-11-22T02:25:00Z',
  confirmed_at: outcome === 'confirmed' ? '2026-11-22T02:25:00Z' : null,
  platform_message_id: null,
  recipients: [{ kind: 'lark_chat', id: 'sample-team', thread_id: null }],
});
const page: ActivityPage = {
  cursor: 'sample-cursor',
  retention_days: 7,
  limit: 50,
  has_more: false,
  items: [
    {
      event_id: 'sample-event',
      kind: 'message',
      source: { type: 'nyxid_relay', platform: 'lark' },
      actor: { id: 'not-displayed', kind: 'human' },
      conversation: { id: 'sample-conversation', kind: 'group' },
      occurred_at: '2026-11-22T02:24:00Z',
      received_at: '2026-11-22T02:24:01Z',
      consumers: [consumer],
    },
  ],
};

it('keeps arrival, absent response and absent delivery distinct', () => {
  const { rerender } = render(
    <ChannelActivityView feed={{ page, connection: 'live', retry: vi.fn() }} />,
  );
  expect(screen.getByText('Received', { exact: true })).toBeVisible();
  expect(screen.getByText('Agent response not confirmed')).toBeVisible();
  expect(screen.getByText('Delivery not confirmed')).toBeVisible();
  expect(screen.queryByText('Agent response produced')).not.toBeInTheDocument();
  expect(screen.queryByText('Delivery confirmed')).not.toBeInTheDocument();
  rerender(
    <ChannelActivityView
      feed={{
        page: { ...page, items: [{ ...page.items[0]!, consumers: [] }] },
        connection: 'live',
        retry: vi.fn(),
      }}
    />,
  );
  expect(
    screen.getByText('No Agent response or recipient delivery receipt is recorded.'),
  ).toBeVisible();
});

it.each(['completed', 'accepted', 'failed', 'unknown'] as const)(
  'does not infer response production or delivery from %s state',
  (state) => {
    render(<ChannelReceiptFacts consumer={{ ...consumer, state }} />);
    expect(screen.getByText('Agent response not confirmed')).toBeVisible();
    expect(screen.getByText('Delivery not confirmed')).toBeVisible();
  },
);

it('shows the production receipt without promoting it to delivery', () => {
  render(
    <ChannelReceiptFacts
      consumer={{ ...consumer, state: 'produced', produced_at: '2026-11-22T02:25:00Z' }}
    />,
  );
  expect(screen.getByText('Agent response produced')).toBeVisible();
  expect(screen.getByText('Delivery not confirmed')).toBeVisible();
  expect(screen.getByText(/This doesn’t mean the reply wasn’t sent/)).toBeVisible();
  expect(screen.queryByText('Delivery confirmed')).not.toBeInTheDocument();
});

it.each([
  ['unknown', 'unknown'],
  ['rejected', 'rejected'],
  ['rate_limited', 'rate limited'],
  ['unsupported', 'unsupported'],
  ['unavailable', 'Delivery unavailable'],
  ['buffered', 'Buffered locally'],
  ['confirmed', 'Delivery confirmed'],
] as const)('keeps the %s delivery receipt and its destination visible', (outcome, label) => {
  render(<ChannelReceiptFacts consumer={{ ...consumer, deliveries: [delivery(outcome)] }} />);
  const receipts = screen.getByRole('list', { name: 'Delivery receipts for Sample task' });
  expect(within(receipts).getByText(label, { exact: true })).toBeVisible();
  expect(within(receipts).getByText('lark_chat: sample-team')).toBeVisible();
  expect(within(receipts).getByText('answer · Original destination')).toBeVisible();
  if (outcome !== 'confirmed')
    expect(screen.queryByText('Delivery confirmed')).not.toBeInTheDocument();
});

it('keeps mixed consumers, recipients, receipt steps and outcomes separate', () => {
  render(
    <ChannelActivityView
      feed={{
        page: {
          ...page,
          items: [
            {
              ...page.items[0]!,
              consumers: [
                {
                  ...consumer,
                  deliveries: [
                    delivery('confirmed'),
                    {
                      ...delivery('unknown'),
                      step: 'tool_reply',
                      destination: 'default',
                      recipients: [
                        { kind: 'lark_chat', id: 'other-team', thread_id: 'thread-two' },
                      ],
                    },
                  ],
                },
                {
                  ...consumer,
                  consumer_id: 'helper',
                  display_name: 'Sample helper',
                  deliveries: [delivery('rejected')],
                },
              ],
            },
          ],
        },
        connection: 'live',
        retry: vi.fn(),
      }}
    />,
  );
  const first = screen.getByRole('region', { name: 'Response and delivery for Sample task' });
  const second = screen.getByRole('region', { name: 'Response and delivery for Sample helper' });
  expect(within(first).getAllByRole('listitem')).toHaveLength(2);
  expect(within(first).getByText('Delivery confirmed')).toBeVisible();
  expect(within(first).getByText('unknown')).toBeVisible();
  expect(within(first).getByText('tool reply · Default destination')).toBeVisible();
  expect(within(first).getByText('lark_chat: other-team · Thread thread-two')).toBeVisible();
  expect(within(second).getByText('rejected')).toBeVisible();
  expect(within(second).queryByText('Delivery confirmed')).not.toBeInTheDocument();
});

it('keeps provider and refusal details available through an expandable disclosure', async () => {
  render(
    <ChannelReceiptFacts
      consumer={{
        ...consumer,
        state: 'refused',
        agent_id: 'agt-synthetic',
        execution_reference: 'exec-synthetic',
        refusal_code: 'event_expired_before_start',
        last_non_admission_code: 'verified_sender_context_unavailable',
      }}
    />,
  );
  const details = screen.getByText('Response details');
  expect(details.parentElement).not.toHaveAttribute('open');
  await userEvent.click(details);
  expect(screen.getByText('agt-synthetic')).toBeVisible();
  expect(screen.getByText('exec-synthetic')).toBeVisible();
  expect(screen.getByText('event_expired_before_start')).toBeVisible();
  expect(screen.getByText('verified_sender_context_unavailable')).toBeVisible();
});

it('distinguishes absent snapshots, confirmed empty history and limited retained history', () => {
  const retry = vi.fn();
  const { rerender } = render(
    <ChannelActivityView feed={{ page: undefined, connection: 'disconnected', retry }} />,
  );
  expect(screen.getByText(/Activity records haven’t loaded/)).toBeVisible();
  expect(screen.queryByText(/No events recorded/)).not.toBeInTheDocument();
  rerender(
    <ChannelActivityView feed={{ page: { ...page, items: [] }, connection: 'live', retry }} />,
  );
  expect(screen.getByText('No events recorded in the last 7 days.')).toBeVisible();
  rerender(
    <ChannelActivityView
      feed={{ page: { ...page, has_more: true }, connection: 'disconnected', retry }}
    />,
  );
  expect(
    screen.getByText('Showing 1 recent events (up to 50; retained for 7 days).'),
  ).toBeVisible();
  expect(screen.getByText('Live activity disconnected')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Refresh activity' }));
  expect(retry).toHaveBeenCalledTimes(1);
  rerender(<ChannelActivityView feed={{ page, connection: 'connecting', retry }} />);
  expect(screen.getByRole('button', { name: 'Refresh activity' })).toBeDisabled();
  expect(screen.getByText('sample-event')).toBeVisible();
});

it('keeps a recorded produced state when its timestamp was not reported', () => {
  render(<ChannelReceiptFacts consumer={{ ...consumer, state: 'produced' }} />);
  expect(screen.getByText('Agent response produced')).toBeVisible();
  expect(screen.getByText('Delivery not confirmed')).toBeVisible();
  expect(screen.queryByText('Agent response not confirmed')).not.toBeInTheDocument();
  expect(document.querySelector('time')).toBeNull();
});
