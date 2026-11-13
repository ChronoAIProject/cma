import { act, render, screen, within } from '@testing-library/react';
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
  snapshot(value: ActivityPage, type = 'activity') {
    this.dispatchEvent(new MessageEvent(type, { data: JSON.stringify(value) }));
  }
}
const snapshot = (state: 'queued' | 'produced' = 'queued'): ActivityPage => ({
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

it('shows the recorded refusal reason without claiming a chat reply', () => {
  vi.stubGlobal('EventSource', Feed);
  render(<ChannelActivity owner="owner" channelId="channel" active />);
  const page = snapshot();
  page.items[0]!.consumers[0]!.state = 'refused';
  page.items[0]!.consumers[0]!.refusal_code = 'event_expired_before_start';
  page.items[0]!.consumers[0]!.last_non_admission_code = 'verified_sender_context_unavailable';
  act(() => Feed.instances[0]!.snapshot(page));
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
  expect(screen.getByText('Build service ·', { exact: false })).toBeVisible();
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
  expect(screen.getByRole('button', { name: 'Reconnect activity' })).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: 'Reconnect activity' }));
  expect(Feed.instances).toHaveLength(2);
  rerender(<ChannelActivity owner="another-owner" channelId="first" active />);
  expect(Feed.instances[1]?.closed).toBe(true);
  expect(screen.queryByText('Response produced', { selector: 'span' })).not.toBeInTheDocument();
  act(() => first.snapshot(snapshot('produced')));
  expect(screen.queryByText('Build service', { exact: false })).not.toBeInTheDocument();
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
  expect(screen.getByRole('button', { name: 'Reconnect activity' })).toBeVisible();
  act(() => feed.snapshot(snapshot('produced')));
  act(() => feed.onerror?.());
  expect(screen.getByText('Queued')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Reconnect activity' })).toBeVisible();
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
  expect(screen.queryByRole('button', { name: 'Reconnect activity' })).not.toBeInTheDocument();
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
  expect(screen.getByText('Accepted by WhatsApp')).toBeVisible();
  expect(screen.getByText('Read by recipient')).toBeVisible();
  page.items[0]!.consumers[0]!.deliveries.push({
    ...delivery,
    step: 'platform_failure',
    outcome: 'failed',
  });
  act(() => Feed.instances[0]!.snapshot(page));
  expect(screen.getByText('WhatsApp could not deliver this message')).toBeVisible();
  expect(screen.getByText('Read by recipient')).toBeVisible();
  expect(Feed.instances).toHaveLength(1);
});
