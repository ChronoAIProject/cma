import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import type { Channel } from './ChannelEditor';
import { ChannelMap } from './ChannelMap';
import { ChannelNodeTests } from './ChannelNodeTests';
import { NodeInspector } from './NodeInspector';
import type { ChannelSelection } from './channelTopology';

afterEach(() => vi.unstubAllGlobals());
const definition = {
  name: 'Team channel',
  sources: [
    { type: 'telegram', bot_id: '100', bot_username: 'team_direct', admission: { type: 'all' } },
    ...['telegram', 'whatsapp', 'lark', 'discord'].map((platform) => ({
      type: 'nyxid_relay',
      platform,
      route_ids: [`${platform}-route`],
      admission: { type: 'all' },
      credentials: 'must-never-appear',
    })),
  ],
  provider: {
    slug: 'cma_codex',
    binding_id: 'binding-default',
    target: { type: 'profile' as const },
  },
  routing: {
    default_consumer: { name: 'assistant', label: 'CMA Agent' },
    consumers: [
      {
        id: 'reviewer',
        name: 'reviewer',
        label: 'Nyxbot reviewer',
        provider: { slug: 'nyxbot', binding_id: 'binding-reviewer' },
      },
      { id: 'custom', name: 'custom', provider: { slug: 'internal-worker' }, pending: true },
    ],
    default_targets: ['default', 'reviewer'],
    rules: [{ id: 'review-route', targets: ['reviewer'] }],
  },
};
const channel: Channel = {
  channel_id: 'channel',
  record_id: 'record',
  version: 5,
  provider: { slug: 'cma_codex', level: 'l2' },
  definition,
  endpoints: {
    custom_event_url: 'https://gateway.example/events',
    nyxid_callback_url: 'https://gateway.example/callback',
    telegram_webhook_url: 'https://gateway.example/telegram',
  },
};
function MapFixture() {
  const [selection, select] = useState<ChannelSelection>({ kind: 'channel' });
  return (
    <>
      <ChannelMap
        channel={channel}
        selection={selection}
        onSelect={select}
        inspectorId="node-inspector"
      />
      <NodeInspector id="node-inspector" channel={channel} selection={selection}>
        <p>Scoped settings</p>
      </NodeInspector>
    </>
  );
}
it('maps all producers and consumers with distinct identities and scoped facts without extra reads', async () => {
  const fetcher = vi.fn();
  vi.stubGlobal('fetch', fetcher);
  const { container } = render(<MapFixture />);
  const producers = screen.getByRole('region', { name: 'Delivery map' });
  const consumers = producers;
  expect(await within(producers).findAllByRole('button', { name: /^Producer:/ })).toHaveLength(5);
  expect(within(consumers).getAllByRole('button', { name: /^Consumer:/ })).toHaveLength(3);
  expect(
    within(producers).getByRole('button', { name: 'Producer: Telegram (Direct webhook)' }),
  ).toBeVisible();
  const relay = within(producers).getByRole('button', { name: 'Producer: Telegram (NyxID relay)' });
  for (const identity of ['telegram', 'whatsapp', 'lark', 'discord'])
    expect(producers.querySelector(`[data-identity="${identity}"] svg`)).not.toBeNull();
  expect(consumers.querySelector('[data-identity="cma"] img')).not.toBeNull();
  expect(consumers.querySelector('[data-identity="nyxbot"] svg')).not.toBeNull();
  expect(within(consumers).getByText('Binding pending')).toBeVisible();
  await userEvent.click(relay);
  expect(relay).toHaveAttribute('aria-pressed', 'true');
  expect(relay).toHaveAttribute('aria-controls', 'node-inspector');
  const inspector = screen.getByRole('region', { name: 'Telegram' });
  expect(within(inspector).getByText('telegram-route')).toBeVisible();
  expect(within(inspector).getByText('https://gateway.example/callback')).toBeVisible();
  expect(within(inspector).queryByText('https://gateway.example/telegram')).not.toBeInTheDocument();
  await userEvent.click(
    within(consumers).getByRole('button', { name: 'Consumer: Nyxbot reviewer (nyxbot)' }),
  );
  expect(screen.getByText('binding-reviewer')).toBeVisible();
  expect(screen.queryByText('binding-default')).not.toBeInTheDocument();
  expect(container.querySelector('pre')).toBeNull();
  expect(container).not.toHaveTextContent('must-never-appear');
  expect(fetcher).not.toHaveBeenCalled();
});
it('keeps keyboard navigation as normal buttons and leaves unknown identities explicit', async () => {
  render(<MapFixture />);
  const unknown = await screen.findByRole('button', { name: 'Consumer: custom (internal-worker)' });
  unknown.focus();
  await userEvent.keyboard('{Enter}');
  expect(unknown).toHaveAttribute('aria-pressed', 'true');
  expect(screen.getByRole('region', { name: 'custom' })).toHaveTextContent('internal-worker');
  expect(screen.getByRole('region', { name: 'custom' })).toHaveTextContent('Binding pending');
});
it('distinguishes a direct WhatsApp producer from its relay without loading credentials', async () => {
  const fetcher = vi.fn();
  vi.stubGlobal('fetch', fetcher);
  render(
    <ChannelMap
      channel={{
        ...channel,
        definition: {
          ...definition,
          sources: [
            { type: 'whatsapp', phone_number_id: '1200123456', credential: 'hidden-value' },
            { type: 'nyxid_relay', platform: 'whatsapp', route_ids: ['team-route'] },
          ],
        },
      }}
      selection={{ kind: 'channel' }}
      onSelect={vi.fn()}
      inspectorId="whatsapp-inspector"
    />,
  );
  const direct = await screen.findByRole('button', {
    name: 'Producer: WhatsApp (Direct WhatsApp webhook)',
  });
  expect(within(direct).getByText('1200123456')).toBeVisible();
  expect(direct.querySelector('[data-identity="whatsapp"] svg')).not.toBeNull();
  expect(screen.getByRole('button', { name: 'Producer: WhatsApp (NyxID relay)' })).toBeVisible();
  expect(screen.queryByText('hidden-value')).not.toBeInTheDocument();
  expect(fetcher).not.toHaveBeenCalled();
});
it('checks only the selected producer and explains the absence of a platform delivery test', async () => {
  const fetcher = vi.fn().mockResolvedValue(
    new Response(
      JSON.stringify({
        source_index: 3,
        platform: 'lark',
        scope: 'configuration_only',
        external_delivery_tested: false,
        checks: [
          { id: 'routes', label: 'Route binding', status: 'passed', detail: '1 route configured' },
        ],
      }),
    ),
  );
  vi.stubGlobal('fetch', fetcher);
  render(
    <ChannelNodeTests
      channelId="channel"
      selection={{ kind: 'producer', index: 3 }}
      denied={vi.fn()}
    />,
  );
  await userEvent.click(screen.getByRole('button', { name: 'Check configuration' }));
  expect(
    await screen.findByText('Configuration check completed. External delivery was not tested.'),
  ).toBeVisible();
  expect(screen.getByText('Passed · 1 route configured')).toBeVisible();
  expect(fetcher).toHaveBeenCalledOnce();
  expect(fetcher.mock.calls[0]![0]).toBe('/api/event-gateway/v1/channels/channel/sources/3/test');
});
it('retains targeted consumer test outcomes when switching nodes without duplicate admission', async () => {
  let finish: (value: Response) => void = () => {};
  const fetcher = vi.fn().mockImplementation(
    () =>
      new Promise<Response>((resolve) => {
        finish = resolve;
      }),
  );
  vi.stubGlobal('fetch', fetcher);
  const denied = vi.fn();
  const { rerender } = render(
    <ChannelNodeTests
      channelId="channel"
      selection={{ kind: 'consumer', id: 'reviewer' }}
      denied={denied}
    />,
  );
  await userEvent.click(screen.getByRole('button', { name: 'Send test event' }));
  expect(screen.getByRole('button', { name: 'Sending test…' })).toBeDisabled();
  rerender(
    <ChannelNodeTests
      channelId="channel"
      selection={{ kind: 'producer', index: 0 }}
      denied={denied}
    />,
  );
  expect(screen.getByRole('button', { name: 'Check configuration' })).toBeEnabled();
  await act(async () => finish(new Response(JSON.stringify({ first_text: 'Reviewer answered' }))));
  expect(screen.queryByText('Reviewer answered')).not.toBeInTheDocument();
  rerender(
    <ChannelNodeTests
      channelId="channel"
      selection={{ kind: 'consumer', id: 'reviewer' }}
      denied={denied}
    />,
  );
  expect(screen.getByText('Reviewer answered')).toBeVisible();
  expect(fetcher).toHaveBeenCalledOnce();
  expect(fetcher.mock.calls[0]![0]).toBe(
    '/api/event-gateway/v1/channels/channel/consumers/reviewer/test',
  );
});
it('shows a completed NyxBot test separately from the provider response', async () => {
  vi.stubGlobal(
    'fetch',
    vi
      .fn()
      .mockResolvedValue(
        new Response(JSON.stringify({ first_text: 'nyxbot', accepted_after_ms: 42 })),
      ),
  );
  render(<ChannelNodeTests channelId="channel" selection={{ kind: 'channel' }} denied={vi.fn()} />);
  await userEvent.click(screen.getByRole('button', { name: 'Send test event' }));
  const status = await screen.findByRole('status');
  expect(within(status).getByText('Test turn completed')).toBeVisible();
  expect(within(status).getByText('Accepted after 42 ms.')).toBeVisible();
  expect(within(status).getByText('Provider response')).toBeVisible();
  expect(within(status).getByText('nyxbot')).toBeVisible();
});

it('keeps lost outcomes unknown and reports denied tests without automatic retries', async () => {
  const fetcher = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('connection lost'))
    .mockResolvedValueOnce(new Response('{}', { status: 403 }));
  vi.stubGlobal('fetch', fetcher);
  const denied = vi.fn();
  render(
    <ChannelNodeTests
      channelId="channel"
      selection={{ kind: 'consumer', id: 'default' }}
      denied={denied}
    />,
  );
  await userEvent.click(screen.getByRole('button', { name: 'Send test event' }));
  expect(
    await screen.findByText(
      'Test result is unknown. Check channel activity before sending another test.',
    ),
  ).toBeVisible();
  expect(fetcher).toHaveBeenCalledOnce();
  await userEvent.click(screen.getByRole('button', { name: 'Send test event' }));
  expect(denied).toHaveBeenCalledWith(403);
  expect(fetcher).toHaveBeenCalledTimes(2);
});

it('does not present a retained test as verification of changed configuration', async () => {
  vi.stubGlobal(
    'fetch',
    vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify({ first_text: 'Saved consumer answered' }))),
  );
  const denied = vi.fn();
  const { rerender } = render(
    <ChannelNodeTests
      channelId="channel"
      channelVersion={3}
      selection={{ kind: 'consumer', id: 'default' }}
      denied={denied}
    />,
  );
  await userEvent.click(screen.getByRole('button', { name: 'Send test event' }));
  await screen.findByText('Saved consumer answered');
  rerender(
    <ChannelNodeTests
      channelId="channel"
      channelVersion={4}
      selection={{ kind: 'consumer', id: 'default' }}
      denied={denied}
    />,
  );
  expect(
    screen.getByText(
      'Configuration changed since this test was requested. Run a new test for the current settings.',
    ),
  ).toBeVisible();
});
