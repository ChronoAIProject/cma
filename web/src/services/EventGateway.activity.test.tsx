import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { EventGateway } from './EventGateway';
import type { Channel } from './ChannelEditor';
import type { ServiceSession } from './client';
import type { ActivityConsumer, ActivityPage } from './useChannelActivity';

class Feed extends EventTarget {
  static CLOSED = 2;
  static instances: Feed[] = [];
  readyState = 1;
  onerror: (() => void) | null = null;
  closed = false;
  constructor(
    readonly url: string,
    readonly options: EventSourceInit,
  ) {
    super();
    Feed.instances.push(this);
  }
  close() {
    this.closed = true;
  }
  snapshot(value: ActivityPage) {
    this.dispatchEvent(new MessageEvent('activity', { data: JSON.stringify(value) }));
  }
}

const channel = (id: string): Channel => ({
  record_id: `${id}-record`,
  channel_id: id,
  version: 1,
  provider: { slug: 'cma_codex', level: 'l2' },
  definition: { name: `${id} channel`, sources: [], schema_version: 1 },
  endpoints: { custom_event_url: '', nyxid_callback_url: '' },
});
const snapshot = (actor: string, state: ActivityConsumer['state'] = 'queued'): ActivityPage => ({
  cursor: `${actor}-${state}`,
  retention_days: 7,
  limit: 50,
  has_more: false,
  items: [
    {
      event_id: `${actor}-event`,
      source: { type: 'custom' },
      actor: { id: 'build-service', display_name: actor, kind: 'service' },
      conversation: { id: 'build-one', kind: 'event' },
      occurred_at: '2026-10-27T00:00:00Z',
      received_at: '2026-10-27T00:00:01Z',
      consumers: [
        {
          consumer_id: 'reviewer',
          display_name: 'Reviewer',
          state,
          dispatched_at: null,
          accepted_at: null,
          produced_at: null,
          completed_at: null,
          response_id: null,
          deliveries: [],
        },
      ],
    },
  ],
});
const session: ServiceSession = {
  authenticated: true,
  subject: 'owner',
  permissions: ['cma-eg:channels:read'],
  expires_at: 9999999999,
};

function setup(active = true) {
  vi.stubGlobal('EventSource', Feed);
  const fetcher = vi
    .fn<typeof fetch>()
    .mockImplementation(async (url) =>
      url === '/api/event-gateway/v1/channels'
        ? new Response(JSON.stringify({ items: [channel('First'), channel('Second')] }))
        : new Response('{}', { status: 404 }),
    );
  vi.stubGlobal('fetch', fetcher);
  const denied = vi.fn();
  return {
    ...render(<EventGateway session={session} denied={denied} active={active} />),
    fetcher,
    denied,
  };
}
const preview = () => screen.getByRole('region', { name: 'Latest channel activity' });
const choose = async (name: string) => {
  const sidebar = screen.getByRole('region', { name: 'Channels' });
  await userEvent.click(await within(sidebar).findByRole('button', { name: new RegExp(name) }));
};
afterEach(() => {
  vi.unstubAllGlobals();
  Feed.instances = [];
});

it('keeps the welcome diagram before selection, then shares one live feed with the map and dialog', async () => {
  const { fetcher } = setup();
  expect(screen.getByRole('region', { name: 'Channel workspace' })).toBeVisible();
  expect(Feed.instances).toHaveLength(0);
  await choose('First channel');
  expect(screen.queryByRole('region', { name: 'Channel workspace' })).not.toBeInTheDocument();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(Feed.instances).toHaveLength(1);
  const feed = Feed.instances[0]!;
  expect(feed.url).toBe('/api/event-gateway/v1/channels/First/activity/stream');
  expect(feed.options).toEqual({ withCredentials: true });
  const latest = preview();
  expect(within(latest).getByText('Waiting for activity snapshot.')).toBeVisible();
  expect(within(latest).queryByText(/No events recorded/)).not.toBeInTheDocument();

  act(() => feed.snapshot(snapshot('Build service')));
  expect(within(latest).getByText('Build service')).toBeVisible();
  expect(within(latest).getByText('custom')).toBeVisible();
  expect(latest.querySelector('time')).toHaveAttribute('datetime', '2026-10-27T00:00:01Z');
  expect(within(latest).getByText('Reviewer · Queued')).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: 'Activity' }));
  const dialog = screen.getByRole('dialog', { name: 'Channel settings' });
  expect(within(dialog).getByText('Build service ·', { exact: false })).toBeVisible();
  expect(within(dialog).getByText('Queued')).toBeVisible();
  expect(Feed.instances).toHaveLength(1);

  act(() => feed.snapshot(snapshot('Build service', 'produced')));
  expect(within(latest).getByText('Reviewer · Response produced')).toBeVisible();
  expect(within(dialog).getByText('Response produced', { selector: 'span' })).toBeVisible();
  await userEvent.click(within(dialog).getByRole('button', { name: 'Close' }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(feed.closed).toBe(false);
  act(() => feed.snapshot(snapshot('Next receipt')));
  expect(within(latest).getByText('Next receipt')).toBeVisible();
  expect(within(latest).queryByText('Build service')).not.toBeInTheDocument();
  expect(Feed.instances).toHaveLength(1);
  expect(fetcher.mock.calls.some(([url]) => String(url).includes('/activity'))).toBe(false);
  expect(fetcher.mock.calls.some(([, init]) => init?.method === 'POST')).toBe(false);
});

it('closes inactive retained streams, ignores their late frames, and uses fresh snapshots on return', async () => {
  const { rerender, unmount, denied } = setup();
  await choose('First channel');
  const first = Feed.instances[0]!;
  act(() => first.snapshot(snapshot('First receipt')));
  await choose('Second channel');
  expect(first.closed).toBe(true);
  expect(Feed.instances).toHaveLength(2);
  expect(within(preview()).queryByText('First receipt')).not.toBeInTheDocument();
  const second = Feed.instances[1]!;
  act(() => second.snapshot(snapshot('Second receipt')));
  act(() => first.snapshot(snapshot('Stale receipt')));
  expect(within(preview()).getByText('Second receipt')).toBeVisible();

  await choose('First channel');
  expect(second.closed).toBe(true);
  expect(Feed.instances).toHaveLength(3);
  expect(within(preview()).getByText('Connecting to live activity…')).toBeVisible();
  expect(within(preview()).getByText('First receipt')).toBeVisible();
  expect(within(preview()).queryByText('Stale receipt')).not.toBeInTheDocument();
  const resumed = Feed.instances[2]!;
  act(() => resumed.snapshot(snapshot('Fresh receipt', 'completed')));
  expect(within(preview()).getByText('Fresh receipt')).toBeVisible();
  expect(within(preview()).getByText('Reviewer · Agent completed')).toBeVisible();

  rerender(<EventGateway session={session} denied={denied} active={false} />);
  expect(resumed.closed).toBe(true);
  act(() => resumed.snapshot(snapshot('Inactive receipt')));
  rerender(<EventGateway session={session} denied={denied} active />);
  expect(Feed.instances).toHaveLength(4);
  expect(within(preview()).queryByText('Inactive receipt')).not.toBeInTheDocument();
  const reactivated = Feed.instances[3]!;
  act(() => reactivated.snapshot({ ...snapshot('unused'), items: [] }));
  expect(within(preview()).getByText('No events recorded in the last 7 days.')).toBeVisible();
  expect(within(preview()).queryByText('Fresh receipt')).not.toBeInTheDocument();
  unmount();
  expect(reactivated.closed).toBe(true);
});

it('shows a receipt without consumers and preserves each reported consumer outcome', async () => {
  setup();
  await choose('First channel');
  const page = snapshot('Build service');
  const event = page.items[0]!;
  const consumer = event.consumers[0]!;
  event.consumers = [];
  act(() => Feed.instances[0]!.snapshot(page));
  expect(within(preview()).getByText('Received', { selector: 'p' })).toBeVisible();
  expect(within(preview()).queryByRole('list')).not.toBeInTheDocument();
  event.consumers = [
    consumer,
    { ...consumer, consumer_id: 'auditor', display_name: 'Auditor', state: 'refused' },
  ];
  act(() => Feed.instances[0]!.snapshot(page));
  expect(within(preview()).getByText('Reviewer · Queued')).toBeVisible();
  expect(within(preview()).getByText('Auditor · Refused')).toBeVisible();
  expect(within(preview()).queryByText('Delivery confirmed')).not.toBeInTheDocument();
});
