import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelEditor, type Channel } from '../ChannelEditor';
import { ChannelMap } from '../ChannelMap';
import { RelayCapabilitiesContext, type RelayDiscovery } from '../RelayCapabilities';
import { channelDraft, channelRequest, object } from './model';
import { nodeCredentials } from './NodeFields';

afterEach(() => vi.unstubAllGlobals());
const available: RelayDiscovery = {
  status: 'ready',
  authoring: {
    native_telegram_message_events_enabled: true,
    native_telegram_topic_lifecycle_enabled: true,
    native_telegram_topic_creation_enabled: true,
    dynamic_sources_writes_enabled: false,
    relay_raw_verification_writes_enabled: false,
    multi_consumer_writes_enabled: true,
    native_bot_mentions_writes_enabled: false,
    thread_contract_writes_enabled: false,
    platforms: [],
  },
};
function fixture(opted = false): Channel {
  return {
    channel_id: 'managed-channel',
    record_id: 'owned-record',
    version: 7,
    provider: { slug: 'cma_codex', level: 'l3' },
    definition: {
      name: 'Managed Telegram',
      connection_ref: 'owned-connection',
      sources: [
        {
          type: 'telegram',
          bot_id: '88',
          bot_username: 'managed_bot',
          admission: { type: 'all' },
          ...(opted ? { topic_lifecycle: true, message_events: true, channel_posts: true } : {}),
        },
      ],
      partition: { version: 1, key: 'conversation_and_sender' },
    },
    endpoints: { custom_event_url: '', nyxid_callback_url: '' },
  };
}
function mockSave(channel: Channel) {
  const fetcher = vi.fn().mockImplementation(async (_url, options) => {
    const body = JSON.parse(options.body);
    return new Response(
      JSON.stringify({
        ...channel,
        version: 8,
        definition: { ...channel.definition, sources: body.sources, name: body.name },
      }),
    );
  });
  vi.stubGlobal('fetch', fetcher);
  return fetcher;
}

it('saves managed Telegram consent on its exact source without credential or identity controls', async () => {
  const channel = fixture(),
    fetcher = mockSave(channel),
    done = vi.fn();
  render(
    <RelayCapabilitiesContext value={available}>
      <ChannelEditor
        channel={channel}
        section={{ kind: 'producer', index: 0 }}
        onDone={done}
        denied={vi.fn()}
      />
    </RelayCapabilitiesContext>,
  );
  expect(screen.queryByLabelText('Bot ID')).not.toBeInTheDocument();
  expect(screen.queryByLabelText('Bot username')).not.toBeInTheDocument();
  expect(screen.queryByLabelText('Telegram bot token')).not.toBeInTheDocument();
  expect(screen.queryByLabelText('Telegram webhook secret')).not.toBeInTheDocument();
  expect(screen.getByText(/managed by this connection/)).toHaveTextContent('88 (@managed_bot)');
  for (const name of [
    'Manage the original forum topic',
    'Create new forum topics',
    'Receive message edits and reactions',
    'Receive channel posts and edits',
  ])
    fireEvent.click(screen.getByLabelText(name));
  fireEvent.click(screen.getByRole('button', { name: 'Save producer' }));
  await waitFor(() => expect(done).toHaveBeenCalledOnce());
  expect(fetcher).toHaveBeenCalledOnce();
  const body = JSON.parse(fetcher.mock.calls[0]![1].body);
  expect(body).toMatchObject({
    expected_version: 7,
    connection_ref: 'owned-connection',
    sources: [
      {
        ...object(channel.definition.sources[0]),
        topic_lifecycle: true,
        topic_creation: true,
        message_events: true,
        channel_posts: true,
      },
    ],
  });
  expect(body.credentials).toBeUndefined();
});

it.each(['all', 'message events', 'channel posts'])(
  'allows managed %s opt-out with authoring disabled and keeps original bot authority',
  async (removed) => {
    const channel = fixture(true),
      fetcher = mockSave(channel),
      done = vi.fn();
    render(
      <ChannelEditor
        channel={channel}
        section={{ kind: 'producer', index: 0 }}
        onDone={done}
        denied={vi.fn()}
      />,
    );
    if (removed === 'all')
      fireEvent.click(screen.getByLabelText('Manage the original forum topic'));
    fireEvent.click(
      screen.getByLabelText(
        removed === 'channel posts'
          ? 'Receive channel posts and edits'
          : 'Receive message edits and reactions',
      ),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Save producer' }));
    await waitFor(() => expect(done).toHaveBeenCalledOnce());
    expect(JSON.parse(fetcher.mock.calls[0]![1].body).sources).toEqual([
      {
        ...object(fixture().definition.sources[0]),
        ...(removed !== 'all' ? { topic_lifecycle: true } : {}),
        ...(removed === 'channel posts' ? { message_events: true } : {}),
      },
    ]);
  },
);

it('keeps saved connection admission when switching back to an unsaved producer draft', async () => {
  const channel = fixture(),
    done = vi.fn();
  const fetcher = vi.fn().mockImplementation(async (_url, options) => {
    const body = JSON.parse(options.body);
    return new Response(
      JSON.stringify({
        ...channel,
        version: 8,
        definition: {
          ...channel.definition,
          sources: body.sources.map((source: object) => ({
            ...source,
            ...(body.connection_admission ? { admission: body.connection_admission } : {}),
          })),
        },
      }),
    );
  });
  vi.stubGlobal('fetch', fetcher);
  const view = (kind: 'producer' | 'channel') => (
    <RelayCapabilitiesContext value={available}>
      <ChannelEditor
        channel={channel}
        section={kind === 'producer' ? { kind, index: 0 } : { kind }}
        onDone={done}
        denied={vi.fn()}
      />
    </RelayCapabilitiesContext>
  );
  const { rerender } = render(view('producer'));
  fireEvent.click(screen.getByLabelText('Manage the original forum topic'));
  rerender(view('channel'));
  fireEvent.click(screen.getByLabelText('Override connection admission'));
  fireEvent.click(screen.getByRole('button', { name: 'Connection admission policy' }));
  fireEvent.click(screen.getByRole('menuitemradio', { name: 'Allowed senders' }));
  fireEvent.click(screen.getByRole('button', { name: 'Save channel' }));
  await waitFor(() => expect(done).toHaveBeenCalledTimes(1));
  rerender(view('producer'));
  expect(screen.getByLabelText('Manage the original forum topic')).toBeChecked();
  fireEvent.click(screen.getByLabelText('Receive message edits and reactions'));
  fireEvent.click(screen.getByRole('button', { name: 'Save producer' }));
  await waitFor(() => expect(done).toHaveBeenCalledTimes(2));
  const body = JSON.parse(fetcher.mock.calls[1]![1].body);
  expect(body.sources[0]).toMatchObject({
    admission: { type: 'allowlist', sender_ids: [] },
    topic_lifecycle: true,
    message_events: true,
  });
  expect(body.connection_admission).toBeUndefined();
});

it('retains managed consent on unrelated channel saves and filters hidden platform credentials', async () => {
  const channel = fixture(true),
    fetcher = mockSave(channel),
    done = vi.fn();
  const draft = channelDraft(channel);
  const hidden = {
    telegram_bot_token: 'hidden-draft',
    telegram_webhook_secret: 'hidden-webhook',
    agent_key: 'provider-draft',
  };
  expect(nodeCredentials(hidden, { kind: 'producer', index: 0 }, draft)).toEqual({});
  expect(channelRequest(draft, hidden, channel).credentials).toEqual({
    agent_key: 'provider-draft',
  });
  render(
    <ChannelEditor
      channel={channel}
      section={{ kind: 'channel' }}
      onDone={done}
      denied={vi.fn()}
    />,
  );
  fireEvent.change(screen.getByLabelText('Channel name'), {
    target: { value: 'Renamed managed channel' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save channel' }));
  await waitFor(() => expect(done).toHaveBeenCalledOnce());
  expect(JSON.parse(fetcher.mock.calls[0]![1].body).sources).toEqual(channel.definition.sources);
});

it('does not offer a second producer on a managed connection', () => {
  render(
    <ChannelMap
      channel={fixture()}
      selection={{ kind: 'channel' }}
      onSelect={vi.fn()}
      inspectorId="inspector"
      canAddProducer
    />,
  );
  expect(screen.queryByRole('button', { name: 'Add producer' })).not.toBeInTheDocument();
});
