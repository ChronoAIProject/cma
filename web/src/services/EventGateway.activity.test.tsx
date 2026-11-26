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
const actorId = 'ou_synthetic_sender_sentinel';
const actorName = 'Synthetic upstream name';
const receiptTimes = new Map<string, string>();
function receivedAt(event: string) {
  if (!receiptTimes.has(event))
    receiptTimes.set(event, new Date(Date.UTC(2026, 9, 27, 0, 0, receiptTimes.size)).toISOString());
  return receiptTimes.get(event)!;
}
const receiptTime = (scope: HTMLElement, event: string) =>
  scope.querySelector(`time[datetime="${receivedAt(event)}"]`);
function assertPrivateActor() {
  expect(document.body.innerHTML).not.toContain(actorId);
  expect(document.body.innerHTML).not.toContain(actorName);
}
const snapshot = (event: string, state: ActivityConsumer['state'] = 'queued'): ActivityPage => ({
  cursor: `${event}-${state}`,
  retention_days: 7,
  limit: 50,
  has_more: false,
  items: [
    {
      event_id: `${event}-event`,
      kind: 'message',
      source: { type: 'native_lark', platform: 'lark' },
      actor: { id: actorId, display_name: actorName, kind: 'human' },
      conversation: { id: 'build-one', kind: 'event' },
      occurred_at: '2026-10-27T00:00:00Z',
      received_at: receivedAt(event),
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
  await screen.findByRole('region', { name: 'Latest channel activity' });
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
  expect(receiptTime(latest, 'Build service')).toBeVisible();
  expect(within(latest).getByText('Message received · lark')).toBeVisible();
  expect(within(latest).getByText('Participant')).toBeVisible();
  assertPrivateActor();
  expect(latest.querySelector('time')).toHaveAttribute('datetime', receivedAt('Build service'));
  expect(within(latest).getByText('Reviewer · Queued')).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: 'Activity' }));
  const dialog = screen.getByRole('dialog', { name: 'Channel settings' });
  expect(within(dialog).getByText('Build service-event')).toBeVisible();
  expect(within(dialog).getByText('Queued')).toBeVisible();
  expect(Feed.instances).toHaveLength(1);

  act(() => feed.snapshot(snapshot('Build service', 'produced')));
  expect(within(latest).getByText('Reviewer · Response produced')).toBeVisible();
  expect(within(dialog).getByText('Response produced', { selector: 'span' })).toBeVisible();
  await userEvent.click(within(dialog).getByRole('button', { name: 'Close' }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(feed.closed).toBe(false);
  act(() => feed.snapshot(snapshot('Next receipt')));
  expect(receiptTime(latest, 'Next receipt')).toBeVisible();
  expect(receiptTime(latest, 'Build service')).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Activity' }));
  expect(screen.getByText('Next receipt-event')).toBeVisible();
  assertPrivateActor();
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
  expect(receiptTime(preview(), 'First receipt')).not.toBeInTheDocument();
  const second = Feed.instances[1]!;
  act(() => second.snapshot(snapshot('Second receipt')));
  act(() => first.snapshot(snapshot('Stale receipt')));
  expect(receiptTime(preview(), 'Second receipt')).toBeVisible();

  await choose('First channel');
  expect(second.closed).toBe(true);
  expect(Feed.instances).toHaveLength(3);
  expect(within(preview()).getByText('Connecting to live activity…')).toBeVisible();
  expect(receiptTime(preview(), 'First receipt')).toBeVisible();
  expect(receiptTime(preview(), 'Stale receipt')).not.toBeInTheDocument();
  const resumed = Feed.instances[2]!;
  act(() => resumed.snapshot(snapshot('Fresh receipt', 'completed')));
  expect(receiptTime(preview(), 'Fresh receipt')).toBeVisible();
  expect(within(preview()).getByText('Reviewer · Agent completed')).toBeVisible();
  assertPrivateActor();

  rerender(<EventGateway session={session} denied={denied} active={false} />);
  expect(resumed.closed).toBe(true);
  act(() => resumed.snapshot(snapshot('Inactive receipt')));
  rerender(<EventGateway session={session} denied={denied} active />);
  expect(Feed.instances).toHaveLength(4);
  expect(receiptTime(preview(), 'Inactive receipt')).not.toBeInTheDocument();
  const reactivated = Feed.instances[3]!;
  act(() => reactivated.snapshot({ ...snapshot('unused'), items: [] }));
  expect(within(preview()).getByText('No events recorded in the last 7 days.')).toBeVisible();
  expect(receiptTime(preview(), 'Fresh receipt')).not.toBeInTheDocument();
  unmount();
  expect(reactivated.closed).toBe(true);
});

it('shows a receipt without consumers and preserves each reported consumer outcome', async () => {
  setup();
  await choose('First channel');
  const page = snapshot('Build service');
  const event = page.items[0]!;
  const consumer = event.consumers[0]!;
  act(() => Feed.instances[0]!.snapshot({ ...page, items: [{ ...event, consumers: [] }] }));
  expect(within(preview()).getByText('Received', { selector: 'p' })).toBeVisible();
  expect(within(preview()).queryByRole('list')).not.toBeInTheDocument();
  const consumers = [
    consumer,
    { ...consumer, consumer_id: 'auditor', display_name: 'Auditor', state: 'refused' as const },
  ];
  act(() => Feed.instances[0]!.snapshot({ ...page, items: [{ ...event, consumers }] }));
  expect(within(preview()).getByText('Reviewer · Queued')).toBeVisible();
  expect(within(preview()).getByText('Auditor · Refused')).toBeVisible();
  expect(within(preview()).queryByText('Delivery confirmed')).not.toBeInTheDocument();
});

it('shows only the latest three receipts beside Activity without adding a subscription', async () => {
  setup();
  await choose('First channel');
  const page = snapshot('Latest receipt');
  const items = ['Latest receipt', 'Second receipt', 'Third receipt', 'Older receipt'].map(
    (name) => snapshot(name).items[0]!,
  );
  act(() => Feed.instances[0]!.snapshot({ ...page, items }));
  const latest = preview();
  expect(receiptTime(latest, 'Latest receipt')).toBeVisible();
  expect(receiptTime(latest, 'Second receipt')).toBeVisible();
  expect(receiptTime(latest, 'Third receipt')).toBeVisible();
  expect(receiptTime(latest, 'Older receipt')).not.toBeInTheDocument();
  const overlay = latest.closest('.delivery-flow__overlay')!;
  expect(within(overlay as HTMLElement).getByRole('button', { name: 'Activity' })).toBeVisible();
  expect(screen.queryByText('Browse delivery nodes')).not.toBeInTheDocument();
  expect(Feed.instances).toHaveLength(1);
});
