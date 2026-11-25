import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelEditor, type Channel, type EditorSection } from '../ChannelEditor';
import { RelayCapabilitiesContext, type RelayDiscovery } from '../RelayCapabilities';
import { channelCredentials, channelRequest, defaultSource } from './model';
import { validateRelayAuthoring } from './relayAuthoring';

const available: RelayDiscovery = {
  status: 'ready',
  authoring: {
    native_whatsapp_writes_enabled: true,
    native_whatsapp_auto_activation_enabled: true,
    native_discord_writes_enabled: true,
    native_discord_auto_activation_enabled: true,
    multi_consumer_writes_enabled: true,
    dynamic_sources_writes_enabled: true,
    relay_raw_verification_writes_enabled: false,
    native_bot_mentions_writes_enabled: false,
    thread_contract_writes_enabled: false,
    platforms: [],
  },
};
const whatsapp = {
  ...defaultSource('whatsapp'),
  app_id: '123',
  waba_id: '456',
  phone_number_id: '789',
};
const bundle = {
  app_secret: 'draft-meta-app-private',
  access_token: 'draft-meta-access-private',
  verify_token: 'draft-meta-webhook-private',
};
function fixture(): Channel {
  return {
    channel_id: 'owned-mixed',
    record_id: 'record-mixed',
    version: 2,
    provider: { slug: 'cma_codex', level: 'l3' },
    definition: {
      name: 'Mixed producers',
      sources: [whatsapp, { ...defaultSource('telegram'), bot_id: '321' }],
      partition: { version: 1, key: 'conversation_and_sender' },
    },
    endpoints: { custom_event_url: '', nyxid_callback_url: '' },
  };
}
function response(channel: Channel) {
  return new Response(JSON.stringify({ ...channel, version: channel.version + 1 }));
}
const enter = (name: string, value: string) =>
  fireEvent.change(screen.getByLabelText(name), { target: { value } });
function enterBundle() {
  enter('Meta app secret', bundle.app_secret);
  enter('Meta access token', bundle.access_token);
  enter('Webhook verification token', bundle.verify_token);
}
afterEach(() => vi.unstubAllGlobals());

it.each([
  ['partial bundle', false, false],
  ['complete bundle', true, false],
  ['disabled WhatsApp capability', true, true],
] as const)(
  'keeps an unsaved %s local when saving another producer',
  async (_label, complete, disabled) => {
    const channel = fixture(),
      done = vi.fn();
    const fetcher = vi.fn().mockImplementation(async () => response(channel));
    vi.stubGlobal('fetch', fetcher);
    const view = (section: EditorSection, discovery = available) => (
      <RelayCapabilitiesContext value={discovery}>
        <ChannelEditor channel={channel} section={section} onDone={done} denied={vi.fn()} />
      </RelayCapabilitiesContext>
    );
    const { rerender } = render(view({ kind: 'producer', index: 0 }));
    if (complete) enterBundle();
    else enter('Meta access token', bundle.access_token);
    const discovery: RelayDiscovery = disabled ? { status: 'legacy' } : available;
    rerender(view({ kind: 'producer', index: 1 }, discovery));
    enter('Telegram bot token', 'draft-telegram-private');
    fireEvent.click(screen.getByRole('button', { name: 'Save producer' }));
    await waitFor(() => expect(done).toHaveBeenCalledOnce());
    expect(fetcher).toHaveBeenCalledOnce();
    const body = JSON.parse(fetcher.mock.calls[0]![1].body);
    expect(body.credentials).toEqual({ telegram_bot_token: 'draft-telegram-private' });
    expect(body.sources).toEqual(channel.definition.sources);
    expect(screen.getByLabelText('Telegram bot token')).toHaveValue('');
    rerender(view({ kind: 'producer', index: 0 }, discovery));
    expect(screen.getByLabelText('Meta access token')).toHaveValue(bundle.access_token);
    expect(screen.getByLabelText('Meta app secret')).toHaveValue(complete ? bundle.app_secret : '');
    expect(screen.getByLabelText('Webhook verification token')).toHaveValue(
      complete ? bundle.verify_token : '',
    );
  },
);

it('keeps uncertain rotation bytes and clears only its submitted fields after retry from another producer', async () => {
  const channel = fixture(),
    done = vi.fn();
  const fetcher = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('Connection interrupted'))
    .mockImplementation(async () => response(channel));
  vi.stubGlobal('fetch', fetcher);
  const view = (section: EditorSection) => (
    <RelayCapabilitiesContext value={available}>
      <ChannelEditor channel={channel} section={section} onDone={done} denied={vi.fn()} />
    </RelayCapabilitiesContext>
  );
  const { rerender } = render(view({ kind: 'producer', index: 1 }));
  enter('Telegram bot token', 'unsaved-telegram-private');
  rerender(view({ kind: 'producer', index: 0 }));
  enterBundle();
  fireEvent.click(screen.getByRole('button', { name: 'Save producer' }));
  await screen.findByRole('button', { name: 'Retry same request' });
  expect(done).not.toHaveBeenCalled();
  expect(screen.getByLabelText('Meta access token')).toHaveValue(bundle.access_token);
  expect(JSON.parse(fetcher.mock.calls[0]![1].body).credentials).toEqual({ whatsapp: bundle });
  rerender(view({ kind: 'producer', index: 1 }));
  expect(screen.getByLabelText('Telegram bot token')).toBeDisabled();
  expect(screen.getByLabelText('Telegram bot token')).toHaveValue('unsaved-telegram-private');
  fireEvent.click(screen.getByRole('button', { name: 'Retry same request' }));
  await waitFor(() => expect(done).toHaveBeenCalledOnce());
  expect(fetcher).toHaveBeenCalledTimes(2);
  expect(fetcher.mock.calls[1]![0]).toBe(fetcher.mock.calls[0]![0]);
  expect(fetcher.mock.calls[1]![1].body).toBe(fetcher.mock.calls[0]![1].body);
  expect(fetcher.mock.calls[1]![1].headers).toEqual(fetcher.mock.calls[0]![1].headers);
  expect(screen.getByLabelText('Telegram bot token')).toHaveValue('unsaved-telegram-private');
  expect(screen.getByLabelText('Telegram bot token')).toBeEnabled();
  rerender(view({ kind: 'producer', index: 0 }));
  for (const name of ['Meta app secret', 'Meta access token', 'Webhook verification token'])
    expect(screen.getByLabelText(name)).toHaveValue('');
});

it.each([
  ['whatsapp', ['whatsapp']],
  ['telegram', ['telegram_bot_token', 'telegram_webhook_secret']],
  ['discord', ['discord_bot_token']],
  ['nyxid_relay', ['channel_key']],
  ['custom', ['hmac_secrets']],
] as const)(
  'sends only credentials for selected %s sources when creating a channel',
  (type, keys) => {
    const credentials = {
      agent_key: 'provider-private',
      whatsapp: bundle,
      telegram_bot_token: 'telegram-private',
      telegram_webhook_secret: 'telegram-webhook-private',
      discord_bot_token: 'discord-private',
      channel_key: 'relay-private',
      replace_hmac: true,
      hmac_secrets: [{ key_id: 'events', secret: btoa('synthetic HMAC material') }],
    };
    const body = channelRequest(
      {
        sources: [
          type === 'custom'
            ? { ...defaultSource(type), auth: { type: 'hmac', key_id: 'events' } }
            : defaultSource(type),
        ],
      },
      credentials,
      undefined,
      'record',
    );
    expect(Object.keys(body.credentials as object).sort()).toEqual(['agent_key', ...keys].sort());
  },
);

it('filters hidden partial WhatsApp drafts before validating creation and omits manual credentials for a managed connection', () => {
  const credentials = {
    agent_key: 'provider-private',
    whatsapp: { access_token: bundle.access_token },
    discord_bot_token: 'discord-private',
    telegram_bot_token: 'telegram-private',
    channel_key: 'relay-private',
    replace_hmac: true,
    hmac_secrets: [{ key_id: 'events', secret: 'hmac-private' }],
  };
  const draft = { sources: [{ ...defaultSource('discord'), bot_id: '77' }] };
  expect(() =>
    validateRelayAuthoring(
      available,
      draft,
      undefined,
      channelCredentials(credentials, draft, true),
    ),
  ).not.toThrow();
  expect(channelRequest(draft, credentials, undefined, 'record').credentials).toEqual({
    agent_key: 'provider-private',
    discord_bot_token: 'discord-private',
  });
  const managed = { ...draft, connection_ref: 'ready-connection' };
  expect(channelRequest(managed, credentials, undefined, 'record').credentials).toEqual({
    agent_key: 'provider-private',
  });
});

it('creates from the visible producer after switching types without submitting hidden credentials', async () => {
  const done = vi.fn(),
    fetcher = vi.fn().mockImplementation(async () => response(fixture()));
  vi.stubGlobal('fetch', fetcher);
  render(
    <RelayCapabilitiesContext value={available}>
      <ChannelEditor ownerSubject="owner" onDone={done} denied={vi.fn()} />
    </RelayCapabilitiesContext>,
  );
  enter('Channel name', 'Fresh Discord');
  enter('Provider agent key', 'provider-private');
  const select = (label: string) => {
    fireEvent.click(screen.getByRole('button', { name: 'Producer 1 connection type' }));
    fireEvent.click(screen.getByRole('menuitemradio', { name: label }));
  };
  select('Direct Telegram bot');
  enter('Telegram bot token', 'hidden-telegram-private');
  enter('Telegram webhook secret', 'hidden-webhook-private');
  select('Direct WhatsApp Business');
  enter('Meta access token', bundle.access_token);
  select('Direct Discord bot');
  enter('Discord bot user ID', '77');
  enter('Discord bot token', 'visible-discord-private');
  fireEvent.click(screen.getByRole('button', { name: 'Create channel' }));
  await waitFor(() => expect(done).toHaveBeenCalledOnce());
  const body = JSON.parse(fetcher.mock.calls[0]![1].body);
  expect(body.credentials).toEqual({
    agent_key: 'provider-private',
    discord_bot_token: 'visible-discord-private',
  });
  expect(body.sources).toHaveLength(1);
  expect(body.sources[0].type).toBe('discord');
});

it('retains explicit shared HMAC keys for an outbound result webhook without a custom producer', () => {
  const draft = {
    sources: [defaultSource('telegram')],
    reply: {
      sinks: [
        { type: 'result_webhook', url: 'https://example.test/result', hmac_key_id: 'result' },
      ],
    },
  };
  const credentials = {
    agent_key: 'provider-private',
    telegram_bot_token: 'telegram-private',
    replace_hmac: true,
    hmac_secrets: [{ key_id: 'result', secret: btoa('synthetic result material') }],
  };
  expect(channelRequest(draft, credentials, undefined, 'record').credentials).toEqual({
    agent_key: 'provider-private',
    telegram_bot_token: 'telegram-private',
    hmac_secrets: credentials.hmac_secrets,
  });
});
