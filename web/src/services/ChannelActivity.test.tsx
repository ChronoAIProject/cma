import { act, fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelActivity } from './ChannelActivity';
import type { ActivityPage } from './useChannelActivity';

class Feed extends EventTarget {
  static CLOSED = 2;
  readyState = 1;
  static instances: Feed[] = [];
  onerror: (() => void) | null = null;
  closed = false;
  constructor(readonly url: string) {
    super();
    Feed.instances.push(this);
  }
  close() {
    this.closed = true;
  }
  snapshot(value: unknown, type = 'activity') {
    this.dispatchEvent(new MessageEvent(type, { data: JSON.stringify(value) }));
  }
}
function expandDetails() {
  for (const summary of screen.getAllByText(/^(Response|Event) details$/)) {
    if (!summary.parentElement?.hasAttribute('open')) fireEvent.click(summary);
  }
}

type Mutable<T> = { -readonly [Key in keyof T]: Mutable<T[Key]> };

const snapshot = (state: 'queued' | 'produced' = 'queued'): Mutable<ActivityPage> => ({
  cursor: state,
  retention_days: 7,
  limit: 50,
  has_more: false,
  items: [
    {
      event_id: 'event-one',
      source: { type: 'custom' },
      actor: { id: 'producer', display_name: 'Build service', kind: 'service' },
      conversation: { id: 'build-one', kind: 'event' },
      occurred_at: '2026-10-27T00:00:00Z',
      received_at: '2026-10-27T00:00:01Z',
      consumers: [
        {
          consumer_id: 'reviewer',
          provider: 'cma_codex',
          state,
          dispatched_at: state === 'produced' ? '2026-10-27T00:00:02Z' : null,
          accepted_at: null,
          produced_at: state === 'produced' ? '2026-10-27T00:00:03Z' : null,
          completed_at: null,
          response_id: null,
          deliveries: [],
        },
      ],
    },
  ],
});

it('labels the bound from confirmed delivery and replaces it when exact production arrives', () => {
  vi.stubGlobal('EventSource', Feed);
  render(<ChannelActivity owner="owner" channelId="channel" active />);
  const page = snapshot('produced');
  const consumer = page.items[0]!.consumers[0]!;
  const exact = consumer.produced_at;
  consumer.state = 'completed';
  consumer.produced_at = null;
  consumer.produced_by = '2026-10-27T00:00:07Z';
  act(() => Feed.instances[0]!.snapshot(page));
  expandDetails();
  expect(
    screen.getByText('Response produced by').parentElement?.querySelector('time'),
  ).toHaveAttribute('datetime', consumer.produced_by);
  expect(screen.getByText(/Confirmed answer delivery/)).toBeVisible();

  consumer.produced_at = exact;
  act(() => Feed.instances[0]!.snapshot({ ...page, cursor: 'exact' }));
  expect(
    screen.getByText('Response produced').parentElement?.querySelector('time'),
  ).toHaveAttribute('datetime', exact);
  expect(screen.queryByText('Response produced by')).not.toBeInTheDocument();
  expect(screen.queryByText(/Confirmed answer delivery/)).not.toBeInTheDocument();
});

it.each(['lark', 'telegram'])(
  'explains an unreported provider Agent on a completed %s delivery',
  (platform) => {
    vi.stubGlobal('EventSource', Feed);
    render(<ChannelActivity owner="owner" channelId="channel" active />);
    const page = snapshot('produced');
    page.items[0]!.source = { type: 'nyxid_relay', platform };
    const consumer = page.items[0]!.consumers[0]!;
    consumer.state = 'completed';
    consumer.conversation_id = 'gateway-conversation';
    consumer.response_id = 'response-one';
    consumer.deliveries = [
      {
        step: 'answer',
        destination: 'origin',
        outcome: 'confirmed',
        observed_at: '2026-10-27T00:00:04Z',
        confirmed_at: '2026-10-27T00:00:04Z',
        platform_message_id: null,
        recipients: [{ kind: `${platform}_chat`, id: 'recipient-one', thread_id: null }],
      },
    ];
    for (const agentId of [undefined, null]) {
      consumer.agent_id = agentId;
      act(() => Feed.instances[0]!.snapshot(page));
      expandDetails();
      const identity = screen.getByText('Provider Agent', { selector: 'dt' }).parentElement!;
      expect(within(identity).getByText('Provider Agent identity was not reported')).toBeVisible();
      expect(screen.queryByText('Provider execution reference')).not.toBeInTheDocument();
      expect(screen.queryByText('Produced by')).not.toBeInTheDocument();
      expect(screen.getByText('Agent completed')).toBeVisible();
      expect(screen.getByText('Delivery confirmed')).toBeVisible();
      expect(screen.getByText('Response produced', { selector: 'dt' })).toBeVisible();
      expect(screen.getByText('gateway-conversation')).toBeVisible();
      expect(screen.getByText('event-one')).toBeVisible();
    }
  },
);

it.each(['lark', 'telegram'])(
  'keeps the reported %s provider Agent through reset and reconnect snapshots',
  async (platform) => {
    vi.stubGlobal('EventSource', Feed);
    render(<ChannelActivity owner="owner" channelId="channel" active />);
    const page = snapshot('produced');
    page.items[0]!.source = { type: 'nyxid_relay', platform };
    page.items[0]!.consumers[0]!.agent_id = 'agt_provider';
    page.items[0]!.consumers[0]!.execution_reference = 'exec_terminal_worker';
    act(() => Feed.instances[0]!.snapshot(page));
    expandDetails();
    const identity = () =>
      within(screen.getByText('Provider Agent', { selector: 'dt' }).parentElement!);
    expect(identity().getByText('agt_provider')).toBeVisible();
    expect(
      within(screen.getByText('Provider execution reference').parentElement!).getByText(
        'exec_terminal_worker',
      ),
    ).toBeVisible();
    expect(screen.queryByText('Provider Agent identity was not reported')).not.toBeInTheDocument();
    act(() => Feed.instances[0]!.snapshot(page, 'reset'));
    expect(identity().getByText('agt_provider')).toBeVisible();
    act(() => {
      Feed.instances[0]!.readyState = Feed.CLOSED;
      Feed.instances[0]!.onerror?.();
    });
    await userEvent.setup().click(screen.getByRole('button', { name: 'Refresh activity' }));
    act(() => Feed.instances[1]!.snapshot(page));
    expect(identity().getByText('agt_provider')).toBeVisible();
    expect(screen.getByText('exec_terminal_worker')).toBeVisible();
    expect(Feed.instances[0]!.closed).toBe(true);
  },
);

it('shows the recorded refusal reason without claiming a chat reply', () => {
  vi.stubGlobal('EventSource', Feed);
  render(<ChannelActivity owner="owner" channelId="channel" active />);
  const page = snapshot();
  page.items[0]!.consumers[0]!.state = 'refused';
  page.items[0]!.consumers[0]!.refusal_code = 'event_expired_before_start';
  page.items[0]!.consumers[0]!.last_non_admission_code = 'verified_sender_context_unavailable';
  act(() => Feed.instances[0]!.snapshot(page));
  expandDetails();
  expect(screen.getByText('Refusal reason')).toBeVisible();
  expect(screen.getByText('event_expired_before_start')).toBeVisible();
  expect(screen.getByText('Last admission refusal')).toBeVisible();
  expect(screen.getByText('verified_sender_context_unavailable')).toBeVisible();
  expect(screen.getByText('No recipient delivery receipt recorded.')).toBeVisible();
});
afterEach(() => {
  vi.unstubAllGlobals();
  Feed.instances = [];
});
it('shows the initial and pushed snapshots without refresh and never calls queued work delivered', async () => {
  vi.stubGlobal('EventSource', Feed);
  const fetcher = vi.fn();
  vi.stubGlobal('fetch', fetcher);
  render(<ChannelActivity owner="owner" channelId="channel" active />);
  expect(Feed.instances[0]?.url).toBe('/api/event-gateway/v1/channels/channel/activity/stream');
  act(() => Feed.instances[0]!.snapshot(snapshot()));
  expect(screen.getByText('Actor not reported ·', { exact: false })).toBeVisible();
  expect(document.body.innerHTML).not.toContain('Build service');
  expect(document.body.innerHTML).not.toContain('producer');
  expect(screen.getByText('Queued')).toBeVisible();
  expect(screen.getByText('No recipient delivery receipt recorded.')).toBeVisible();
  expect(screen.queryByText('Delivery confirmed')).not.toBeInTheDocument();
  act(() => Feed.instances[0]!.snapshot(snapshot('produced')));
  expect(screen.getByText('Response produced', { selector: 'span' })).toBeVisible();
  expect(screen.queryByText('Queued')).not.toBeInTheDocument();
  expect(fetcher).not.toHaveBeenCalled();
  expect(Feed.instances).toHaveLength(1);
});
it('preserves records on disconnect, explicitly reconnects, and ignores old channel frames', async () => {
  vi.stubGlobal('EventSource', Feed);
  const { rerender, unmount } = render(<ChannelActivity owner="owner" channelId="first" active />);
  const first = Feed.instances[0]!;
  act(() => first.snapshot(snapshot()));
  act(() => first.onerror?.());
  expect(first.closed).toBe(false);
  expect(screen.getByText('Reconnecting live activity…')).toBeVisible();
  act(() => first.snapshot(snapshot('produced')));
  expect(screen.getByText('Response produced', { selector: 'span' })).toBeVisible();
  first.readyState = Feed.CLOSED;
  act(() => first.onerror?.());
  expect(first.closed).toBe(true);
  expect(screen.getByText('Response produced', { selector: 'span' })).toBeVisible();
  act(() => first.snapshot(snapshot()));
  expect(screen.queryByText('Queued')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Refresh activity' })).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: 'Refresh activity' }));
  expect(Feed.instances).toHaveLength(2);
  rerender(<ChannelActivity owner="another-owner" channelId="first" active />);
  expect(Feed.instances[1]?.closed).toBe(true);
  expect(screen.queryByText('Response produced', { selector: 'span' })).not.toBeInTheDocument();
  act(() => first.snapshot(snapshot('produced')));
  expect(screen.queryByText('event-one')).not.toBeInTheDocument();
  const page = snapshot('produced');
  page.items[0]!.consumers[0]!.deliveries = [
    {
      step: 'answer',
      destination: 'origin',
      outcome: 'confirmed',
      observed_at: '2026-10-27T00:00:04Z',
      confirmed_at: '2026-10-27T00:00:04Z',
      platform_message_id: 'message-one',
      recipients: [{ kind: 'custom_conversation', id: 'build-one', thread_id: null }],
    },
  ];
  act(() => Feed.instances[2]!.snapshot(page, 'reset'));
  expect(screen.getByText('Delivery confirmed')).toBeVisible();
  expect(
    within(screen.getByRole('article')).getByText('custom_conversation: build-one', {
      exact: false,
    }),
  ).toBeVisible();
  unmount();
  expect(Feed.instances[2]?.closed).toBe(true);
});
it('does not subscribe while its owning channel view is inactive', () => {
  vi.stubGlobal('EventSource', Feed);
  const { rerender } = render(<ChannelActivity owner="owner" channelId="channel" active={false} />);
  expect(Feed.instances).toHaveLength(0);
  rerender(<ChannelActivity owner="owner" channelId="channel" active />);
  expect(Feed.instances).toHaveLength(1);
  rerender(<ChannelActivity owner="owner" channelId="channel" active={false} />);
  expect(Feed.instances[0]?.closed).toBe(true);
});

it('retains the last authoritative evidence and offers retry for malformed snapshots', async () => {
  vi.stubGlobal('EventSource', Feed);
  render(<ChannelActivity owner="owner" channelId="channel" active />);
  const feed = Feed.instances[0]!;
  act(() => feed.snapshot(snapshot()));
  act(() =>
    feed.dispatchEvent(new MessageEvent('activity', { data: '{"items":[{}],"cursor":"broken"}' })),
  );
  expect(feed.closed).toBe(true);
  expect(screen.getByText('Queued')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Refresh activity' })).toBeVisible();
  act(() => feed.snapshot(snapshot('produced')));
  act(() => feed.onerror?.());
  expect(screen.getByText('Queued')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Refresh activity' })).toBeVisible();
});

it.each(['activity', 'reset'])(
  'rejects a malformed source in an %s frame and reconnects',
  async (type) => {
    vi.stubGlobal('EventSource', Feed);
    render(<ChannelActivity owner="owner" channelId="channel" active />);
    const feed = Feed.instances[0]!;
    act(() => feed.snapshot(snapshot()));
    const receipt = screen.getByRole('article');
    const replacement = snapshot('produced');
    const next = { ...replacement.items[0]!, event_id: 'event-two' };
    act(() =>
      feed.snapshot(
        {
          ...replacement,
          items: [
            next,
            {
              ...next,
              event_id: 'malformed-event',
              source: { type: 'nyxid_relay', platform: { name: 'telegram' } },
            },
          ],
        },
        type,
      ),
    );

    expect(feed.closed).toBe(true);
    expect(screen.getByRole('article')).toBe(receipt);
    expect(screen.getByText('Queued')).toBeVisible();
    expect(screen.queryByText('event-two')).not.toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveTextContent('Live activity disconnected');
    await userEvent.click(screen.getByRole('button', { name: 'Refresh activity' }));
    expect(Feed.instances).toHaveLength(2);
    act(() => Feed.instances[1]!.snapshot({ ...replacement, items: [next] }));
    expect(screen.getByRole('status')).toHaveTextContent('Live activity');
    expect(screen.getByText('event-two')).toBeVisible();
    expect(screen.queryByText('event-one')).not.toBeInTheDocument();
  },
);

it('renders an older snapshot with optional receipt fields omitted', () => {
  vi.stubGlobal('EventSource', Feed);
  render(<ChannelActivity owner="owner" channelId="channel" active />);
  const { items, ...page } = snapshot();
  const { consumers, ...event } = items[0]!;
  const { consumer_id, state } = consumers[0]!;
  act(() =>
    Feed.instances[0]!.snapshot({
      ...page,
      items: [
        {
          ...event,
          consumers: [
            {
              consumer_id,
              state,
              deliveries: [
                {
                  step: 'answer',
                  destination: 'origin',
                  outcome: 'unknown',
                  observed_at: event.received_at,
                },
              ],
            },
          ],
        },
      ],
    }),
  );
  expect(screen.getByRole('status')).toHaveTextContent('Live activity');
  expect(screen.getByText('Event received · custom')).toBeVisible();
  expect(screen.getByText('Recipient not reported')).toBeVisible();
});

it('keeps native recoverable reconnects alive beyond five errors and accepts the next snapshot', () => {
  vi.stubGlobal('EventSource', Feed);
  render(<ChannelActivity owner="owner" channelId="channel" active />);
  const feed = Feed.instances[0]!;
  act(() => feed.snapshot(snapshot()));
  feed.readyState = 0;
  for (let attempt = 0; attempt < 8; attempt += 1) {
    act(() => feed.onerror?.());
    expect(feed.closed).toBe(false);
    expect(screen.getByText('Reconnecting live activity…')).toBeVisible();
  }
  expect(screen.getByText('Queued')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Refresh activity' })).toBeDisabled();
  act(() => feed.snapshot(snapshot('produced')));
  expect(screen.getByText('Live activity')).toBeVisible();
  expect(screen.getByText('Response produced', { selector: 'span' })).toBeVisible();
  expect(Feed.instances).toHaveLength(1);
});

it('distinguishes local buffering from confirmed recipient delivery', () => {
  vi.stubGlobal('EventSource', Feed);
  render(<ChannelActivity owner="owner" channelId="channel" active />);
  const page = snapshot('produced');
  page.items[0]!.consumers[0]!.deliveries = [
    {
      step: 'answer',
      destination: 'origin',
      outcome: 'buffered',
      observed_at: '2026-10-27T00:00:04Z',
      confirmed_at: null,
      platform_message_id: null,
      recipients: [],
    },
  ];
  act(() => Feed.instances[0]!.snapshot(page));
  expandDetails();
  expect(screen.getByText('Buffered locally')).toBeVisible();
  expect(screen.queryByText('Delivery confirmed')).not.toBeInTheDocument();
  expect(screen.getByText('Recipient not reported', { exact: false })).toBeVisible();
});

it('shows asynchronous WhatsApp recipient outcomes separately from API acceptance', () => {
  vi.stubGlobal('EventSource', Feed);
  render(<ChannelActivity owner="owner" channelId="channel" active />);
  const page = snapshot('produced');
  const delivery = {
    step: 'answer' as const,
    destination: 'origin' as const,
    outcome: 'confirmed' as const,
    observed_at: '2026-10-27T00:00:04Z',
    confirmed_at: '2026-10-27T00:00:04Z',
    platform_message_id: 'wamid.reply',
    recipients: [{ kind: 'whatsapp_chat', id: '447700900123', thread_id: null }],
  };
  page.items[0]!.consumers[0]!.deliveries = [delivery];
  act(() => Feed.instances[0]!.snapshot(page));
  expandDetails();
  expect(screen.getByText('Accepted by WhatsApp')).toBeVisible();
  expect(screen.queryByText('Delivered to recipient')).not.toBeInTheDocument();
  page.items[0]!.consumers[0]!.deliveries.push({
    ...delivery,
    step: 'platform_status',
    outcome: 'read',
    observed_at: '2026-10-27T00:00:09Z',
    confirmed_at: '2026-10-27T00:00:07Z',
  });
  act(() => Feed.instances[0]!.snapshot(page));
  expandDetails();
  expect(screen.getByText('Accepted by WhatsApp')).toBeVisible();
  expect(screen.getByText('Read by recipient')).toBeVisible();
  page.items[0]!.consumers[0]!.deliveries.push({
    ...delivery,
    step: 'platform_failure',
    outcome: 'failed',
  });
  act(() => Feed.instances[0]!.snapshot(page));
  expandDetails();
  expect(screen.getByText('WhatsApp could not deliver this message')).toBeVisible();
  expect(screen.getByText('Read by recipient')).toBeVisible();
  expect(Feed.instances).toHaveLength(1);
});

it('refreshes the same read-only stream once and disables refresh until a snapshot arrives', async () => {
  vi.stubGlobal('EventSource', Feed);
  const fetcher = vi.fn();
  vi.stubGlobal('fetch', fetcher);
  const { rerender } = render(<ChannelActivity owner="owner" channelId="first" active />);
  act(() => Feed.instances[0]!.snapshot(snapshot()));
  await userEvent.click(screen.getByRole('button', { name: 'Refresh activity' }));
  expect(Feed.instances[0]!.closed).toBe(true);
  expect(Feed.instances[1]!.url).toBe(Feed.instances[0]!.url);
  expect(screen.getByRole('button', { name: 'Refresh activity' })).toBeDisabled();
  expect(screen.getByText('event-one')).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: 'Refresh activity' }));
  expect(Feed.instances).toHaveLength(2);
  act(() => Feed.instances[1]!.snapshot(snapshot('produced')));
  expect(screen.getByRole('button', { name: 'Refresh activity' })).toBeEnabled();
  rerender(<ChannelActivity owner="owner" channelId="second" active />);
  expect(Feed.instances[1]!.closed).toBe(true);
  expect(screen.queryByText('event-one')).not.toBeInTheDocument();
  act(() => Feed.instances[1]!.snapshot(snapshot()));
  expect(screen.queryByText('event-one')).not.toBeInTheDocument();
  expect(Feed.instances[2]!.url).toBe('/api/event-gateway/v1/channels/second/activity/stream');
  expect(fetcher).not.toHaveBeenCalled();
});
