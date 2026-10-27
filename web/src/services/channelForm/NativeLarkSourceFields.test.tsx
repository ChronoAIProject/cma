import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelEditor, type Channel, type EditorSection } from '../ChannelEditor';
import { ChannelMap } from '../ChannelMap';
import { NodeInspector } from '../NodeInspector';
import {
  RelayCapabilitiesContext,
  nativeLarkUnavailable,
  type RelayDiscovery,
} from '../RelayCapabilities';
import { NodeFields, nodeCredentials } from './NodeFields';
import { SourceFields } from './SourceFields';
import { ChannelPolicyFields } from './ChannelPolicyFields';
import { channelRequest, credentialsPatch, defaultSource } from './model';
import { validateRelayAuthoring } from './relayAuthoring';

const available: RelayDiscovery = {
  status: 'ready',
  authoring: {
    native_lark_writes_enabled: true,
    native_lark_intake_enabled: true,
    multi_consumer_writes_enabled: true,
    dynamic_sources_writes_enabled: true,
    relay_raw_verification_writes_enabled: false,
    native_bot_mentions_writes_enabled: true,
    thread_contract_writes_enabled: true,
    platforms: [],
  },
};
const identity = {
  region: 'feishu',
  app_id: 'cli_separate_app',
  tenant_key: 'tenant_one',
  bot_open_id: 'ou_separate_bot',
};
const source = {
  ...defaultSource('native_lark'),
  separate_app: true,
  identity,
  admission: { type: 'allowlist', sender_ids: ['ou_allowed_human'] },
};
const bundle = {
  app_secret: 'sample-app-private-value',
  encrypt_key: 'sample-encrypt-private-value',
  verification_token: 'sample-verify-private-value',
};
const channel: Channel = {
  channel_id: 'owned-native-lark',
  record_id: '11111111-1111-4111-8111-111111111111',
  version: 2,
  provider: { slug: 'cma_codex', level: 'l3' },
  definition: {
    name: 'Separate app',
    sources: [source],
    partition: { version: 1, key: 'conversation' },
  },
  endpoints: {
    custom_event_url: '',
    nyxid_callback_url: 'https://example.test/relay',
    native_lark_webhook_url: 'https://example.test/native-lark/owned',
  },
};
const enter = (name: string, value: string) =>
  fireEvent.change(screen.getByLabelText(name), { target: { value } });
const enterBundle = () => {
  enter('App secret', bundle.app_secret);
  enter('Encrypt Key', bundle.encrypt_key);
  enter('Verification Token', bundle.verification_token);
};
const response = (value = channel) => new Response(JSON.stringify({ ...value, version: 3 }));
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

it.each(['loading', 'legacy', 'failed'] as const)(
  'requires explicit advertised setup for %s discovery',
  (status) => {
    expect(nativeLarkUnavailable({ status })).toBeTruthy();
    expect(() =>
      validateRelayAuthoring({ status }, { sources: [source] }, undefined, { native_lark: bundle }),
    ).toThrow();
  },
);
it('requires both gates for new sources and permits retained edits with intake paused', () => {
  const paused = {
    ...available,
    authoring: { ...available.authoring!, native_lark_intake_enabled: false },
  };
  expect(() =>
    validateRelayAuthoring(paused, { sources: [source] }, undefined, { native_lark: bundle }),
  ).toThrow('message delivery');
  expect(() =>
    validateRelayAuthoring(
      paused,
      { sources: [source] },
      { sources: [source] },
      { native_lark: bundle },
    ),
  ).not.toThrow();
  expect(() => validateRelayAuthoring(available, { sources: [source] })).toThrow('all three');
  expect(() =>
    validateRelayAuthoring(available, { sources: [source] }, undefined, { native_lark: bundle }),
  ).not.toThrow();
});
it('retains unchanged sources and blank credentials with writes disabled; blocks changes and rotation', () => {
  const blank = { native_lark: { app_secret: '', encrypt_key: '', verification_token: '' } };
  expect(credentialsPatch(blank, false)).toBeUndefined();
  expect(() =>
    validateRelayAuthoring(
      { status: 'legacy' },
      { sources: [source] },
      { sources: [source] },
      blank,
    ),
  ).not.toThrow();
  expect(() =>
    validateRelayAuthoring(
      { status: 'legacy' },
      { sources: [source] },
      { sources: [source] },
      { native_lark: bundle },
    ),
  ).toThrow('not enabled');
  expect(() =>
    validateRelayAuthoring(
      { status: 'legacy' },
      { sources: [{ ...source, identity: { ...identity, tenant_key: 'tenant_other' } }] },
      { sources: [source] },
    ),
  ).toThrow('not enabled');
});
it.each([
  { region: 'other' },
  { app_id: 'another-app' },
  { bot_open_id: 'user-one' },
  { tenant_key: 'with spaces' },
  { app_id: 'cli_' + 'a'.repeat(125) },
])('refuses invalid exact identity %j', (invalid) => {
  expect(() =>
    validateRelayAuthoring(
      available,
      { sources: [{ ...source, identity: { ...identity, ...invalid } }] },
      undefined,
      { native_lark: bundle },
    ),
  ).toThrow('exact');
});
it('requires separate-app acknowledgment and refuses bot intake, thread following and duplicates', () => {
  expect(defaultSource('native_lark').separate_app).toBe(false);
  for (const [next, message] of [
    [{ sources: [{ ...source, separate_app: false }] }, 'separate app'],
    [
      { sources: [{ ...source, admission: { type: 'scoped', bot_messages: 'native_mentions' } }] },
      'human messages',
    ],
    [{ sources: [source], reply: { thread_contract: { version: 1 } } }, 'thread following'],
    [{ sources: [source, source] }, 'only one'],
  ] as const)
    expect(() =>
      validateRelayAuthoring(available, next, undefined, { native_lark: bundle }),
    ).toThrow(message);
});
it('accepts complete bounded bundles without trimming or substituting secret bytes', () => {
  expect(credentialsPatch({ native_lark: bundle }, false)).toEqual({ native_lark: bundle });
  for (const native_lark of [
    { app_secret: bundle.app_secret },
    { ...bundle, encrypt_key: 'short' },
    { ...bundle, verification_token: 'x'.repeat(513) },
    { ...bundle, app_secret: ' leading-whitespace' },
  ])
    expect(() => credentialsPatch({ native_lark }, false)).toThrow();
});
it('isolates credentials by producer and excludes hidden drafts from creation and consumer saves', () => {
  const credentials = {
    agent_key: 'provider-private',
    native_lark: bundle,
    channel_key: 'relay-private',
    telegram_bot_token: 'telegram-private',
  };
  expect(
    nodeCredentials(credentials, { kind: 'consumer', id: 'default' }, { sources: [source] }),
  ).toEqual({ agent_key: 'provider-private' });
  expect(
    nodeCredentials(credentials, { kind: 'producer', index: 0 }, { sources: [source] }),
  ).toEqual({ native_lark: bundle });
  expect(
    channelRequest({ sources: [source] }, credentials, undefined, channel.record_id).credentials,
  ).toEqual({ agent_key: 'provider-private', native_lark: bundle });
  expect(
    channelRequest(
      { sources: [defaultSource('telegram')] },
      { ...credentials, native_lark: { app_secret: 'partial' } },
      undefined,
      channel.record_id,
    ).credentials,
  ).toEqual({ agent_key: 'provider-private', telegram_bot_token: 'telegram-private' });
  expect(
    channelRequest(
      { sources: [source], connection_ref: 'managed-telegram' },
      credentials,
      undefined,
      channel.record_id,
    ).credentials,
  ).toEqual({ agent_key: 'provider-private' });
});
it('shows exact app identity and human admission without relay resources, forms or bot controls', () => {
  const onChange = vi.fn();
  render(
    <RelayCapabilitiesContext value={available}>
      <SourceFields
        value={{
          ...source,
          admission: {
            type: 'scoped',
            senders: { type: 'open' },
            chats: { type: 'open' },
            groups: 'mention',
          },
        }}
        onChange={onChange}
      />
    </RelayCapabilitiesContext>,
  );
  expect(screen.getByLabelText('App ID')).toHaveValue(identity.app_id);
  expect(screen.getByLabelText('Tenant key')).toHaveValue(identity.tenant_key);
  expect(screen.getByLabelText('Bot open ID')).toHaveValue(identity.bot_open_id);
  expect(screen.getByRole('button', { name: 'App region' })).toHaveTextContent('Feishu');
  expect(screen.getByRole('checkbox', { name: /This is a separate app/ })).toBeChecked();
  for (const label of [
    'NyxID issuer',
    'Messages from bots',
    'Read embedded media',
    'Enable verified thread following',
  ])
    expect(screen.queryByLabelText(label)).not.toBeInTheDocument();
  enter('Tenant key', 'tenant_two');
  expect(onChange).toHaveBeenCalledWith(
    expect.objectContaining({ identity: { ...identity, tenant_key: 'tenant_two' } }),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Group messages' }));
  expect(screen.queryByRole('menuitemradio', { name: 'reply to bot' })).not.toBeInTheDocument();
});
it('gates producer selection and places all masked credentials beside the producer', () => {
  const view = (discovery: RelayDiscovery) => (
    <RelayCapabilitiesContext value={discovery}>
      <NodeFields
        draft={{ sources: [source] }}
        onChange={vi.fn()}
        credentials={{}}
        onCredentials={vi.fn()}
        section={{ kind: 'channel' }}
        create
        sourceCount={0}
        denied={vi.fn()}
      />
    </RelayCapabilitiesContext>
  );
  const { rerender } = render(view({ status: 'legacy' }));
  for (const label of ['App secret', 'Encrypt Key', 'Verification Token'])
    expect(screen.getByLabelText(label)).toHaveAttribute('type', 'password');
  for (const label of [
    'NyxID channel key',
    'Telegram bot token',
    'Enable verified thread following',
  ])
    expect(screen.queryByLabelText(label)).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Producer 1 connection type' }));
  expect(screen.getByRole('menuitemradio', { name: /^Direct Lark \/ Feishu app/ })).toHaveAttribute(
    'aria-disabled',
    'true',
  );
  fireEvent.keyDown(screen.getByRole('menuitemradio', { name: /^Direct Lark \/ Feishu app/ }), {
    key: 'Escape',
  });
  rerender(view(available));
  fireEvent.click(screen.getByRole('button', { name: 'Producer 1 connection type' }));
  expect(
    screen.getByRole('menuitemradio', { name: /^Direct Lark \/ Feishu app/ }),
  ).not.toHaveAttribute('aria-disabled', 'true');
});
it('preserves existing relay thread configuration without offering native thread following', () => {
  const change = vi.fn();
  const { rerender } = render(
    <ChannelPolicyFields value={{ sources: [source] }} onChange={change} />,
  );
  expect(screen.queryByLabelText('Enable verified thread following')).not.toBeInTheDocument();
  rerender(
    <ChannelPolicyFields
      value={{
        sources: [{ type: 'nyxid_relay', platform: 'feishu', bot_id: 'ou_relay' }],
        reply: { thread_contract: { version: 1, follow_chat_ids: ['oc_saved'] } },
      }}
      onChange={change}
    />,
  );
  expect(screen.getByLabelText('Enable verified thread following')).toBeChecked();
  expect(screen.getByLabelText('Follow chat IDs 1')).toHaveValue('oc_saved');
  expect(change).not.toHaveBeenCalled();
});
it('submits one atomic rotation and clears confirmed secret fields', async () => {
  const fetcher = vi.fn().mockImplementation(async () => response());
  vi.stubGlobal('fetch', fetcher);
  render(
    <RelayCapabilitiesContext value={available}>
      <ChannelEditor
        channel={channel}
        section={{ kind: 'producer', index: 0 }}
        onDone={vi.fn()}
        denied={vi.fn()}
      />
    </RelayCapabilitiesContext>,
  );
  enterBundle();
  fireEvent.click(screen.getByRole('button', { name: 'Save producer' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  expect(JSON.parse(fetcher.mock.calls[0]![1].body)).toMatchObject({
    expected_version: 2,
    sources: [source],
    credentials: { native_lark: bundle },
  });
  await waitFor(() => expect(screen.getByLabelText('App secret')).toHaveValue(''));
  expect(screen.getByLabelText('Encrypt Key')).toHaveValue('');
  expect(screen.getByLabelText('Verification Token')).toHaveValue('');
});
it.each([false, true])(
  'retains an unrelated %s complete draft while another producer saves with gates off',
  async (complete) => {
    const mixed = {
      ...channel,
      definition: {
        ...channel.definition,
        sources: [source, { ...defaultSource('telegram'), bot_id: '321' }],
      },
    };
    const fetcher = vi.fn().mockImplementation(async () => response(mixed));
    vi.stubGlobal('fetch', fetcher);
    const view = (section: EditorSection, discovery = available) => (
      <RelayCapabilitiesContext value={discovery}>
        <ChannelEditor channel={mixed} section={section} onDone={vi.fn()} denied={vi.fn()} />
      </RelayCapabilitiesContext>
    );
    const { rerender } = render(view({ kind: 'producer', index: 0 }));
    if (complete) enterBundle();
    else enter('Encrypt Key', bundle.encrypt_key);
    enter('Tenant key', 'unsaved_tenant');
    rerender(view({ kind: 'producer', index: 1 }, { status: 'legacy' }));
    enter('Telegram bot token', 'telegram-private');
    fireEvent.click(screen.getByRole('button', { name: 'Save producer' }));
    await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
    expect(JSON.parse(fetcher.mock.calls[0]![1].body)).toMatchObject({
      sources: mixed.definition.sources,
      credentials: { telegram_bot_token: 'telegram-private' },
    });
    await waitFor(() => expect(screen.getByLabelText('Telegram bot token')).toHaveValue(''));
    rerender(view({ kind: 'producer', index: 0 }));
    expect(screen.getByLabelText('Encrypt Key')).toHaveValue(bundle.encrypt_key);
    expect(screen.getByLabelText('Tenant key')).toHaveValue('unsaved_tenant');
  },
);
it('edits channel metadata with disabled native writes while keeping saved source and credentials', async () => {
  const fetcher = vi.fn().mockImplementation(async () => response());
  vi.stubGlobal('fetch', fetcher);
  render(
    <RelayCapabilitiesContext value={{ status: 'legacy' }}>
      <ChannelEditor
        channel={channel}
        section={{ kind: 'channel' }}
        onDone={vi.fn()}
        denied={vi.fn()}
      />
    </RelayCapabilitiesContext>,
  );
  enter('Channel name', 'Renamed channel');
  fireEvent.click(screen.getByRole('button', { name: 'Save channel' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  const body = JSON.parse(fetcher.mock.calls[0]![1].body);
  expect(body.name).toBe('Renamed channel');
  expect(body.sources).toEqual([source]);
  expect(body.credentials).toBeUndefined();
});
it('creates an exact separate app without submitting hidden relay credentials', async () => {
  const fetcher = vi.fn().mockImplementation(async () => response());
  vi.stubGlobal('fetch', fetcher);
  render(
    <RelayCapabilitiesContext value={available}>
      <ChannelEditor ownerSubject="owner" onDone={vi.fn()} denied={vi.fn()} />
    </RelayCapabilitiesContext>,
  );
  enter('Channel name', 'New app');
  enter('Provider agent key', 'provider-private');
  enter('NyxID channel key', 'hidden-relay-private');
  fireEvent.click(screen.getByRole('button', { name: 'Producer 1 connection type' }));
  fireEvent.click(screen.getByRole('menuitemradio', { name: /^Direct Lark \/ Feishu app/ }));
  enter('App ID', identity.app_id);
  enter('Tenant key', identity.tenant_key);
  enter('Bot open ID', identity.bot_open_id);
  fireEvent.click(screen.getByRole('checkbox', { name: /This is a separate app/ }));
  fireEvent.click(screen.getByRole('button', { name: 'Add Allowed sender IDs' }));
  enter('Allowed sender IDs 1', 'ou_allowed_human');
  enterBundle();
  fireEvent.click(screen.getByRole('button', { name: 'Create channel' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  const body = JSON.parse(fetcher.mock.calls[0]![1].body);
  expect(body.sources).toEqual([{ ...source, identity: { ...identity, region: 'lark' } }]);
  expect(body.credentials).toEqual({ agent_key: 'provider-private', native_lark: bundle });
  expect(body.record_id).toMatch(/^[0-9a-f-]{36}$/);
});
it.each(['lark', 'feishu'])(
  'uses the %s icon and native label and copies only its endpoint',
  async (region) => {
    const fetcher = vi.fn();
    vi.stubGlobal('fetch', fetcher);
    const copy = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: copy },
    });
    const platform = region === 'lark' ? 'Lark' : 'Feishu';
    const current = {
      ...channel,
      definition: {
        ...channel.definition,
        sources: [
          { ...source, identity: { ...identity, region } },
          { type: 'nyxid_relay', platform: region, route_ids: ['saved-route'] },
        ],
      },
    };
    render(
      <>
        <ChannelMap
          channel={current}
          selection={{ kind: 'producer', index: 0 }}
          onSelect={vi.fn()}
          inspectorId="lark-inspector"
        />
        <NodeInspector
          channel={current}
          selection={{ kind: 'producer', index: 0 }}
          id="lark-inspector"
        >
          <span>Settings</span>
        </NodeInspector>
      </>,
    );
    const direct = await screen.findByRole('button', {
      name: `Producer: ${platform} (Direct ${platform} app)`,
    });
    expect(direct.querySelector(`[data-identity="${region}"] svg`)).not.toBeNull();
    expect(
      screen.getByRole('button', { name: `Producer: ${platform} (NyxID relay)` }),
    ).toBeVisible();
    const inspector = screen.getByRole('region', { name: platform });
    expect(within(inspector).getByText(identity.tenant_key)).toBeVisible();
    expect(
      within(inspector).queryByText(channel.endpoints.nyxid_callback_url),
    ).not.toBeInTheDocument();
    fireEvent.click(within(inspector).getByRole('button', { name: 'Copy Lark / Feishu webhook' }));
    await waitFor(() =>
      expect(copy).toHaveBeenCalledWith(channel.endpoints.native_lark_webhook_url),
    );
    expect(fetcher).not.toHaveBeenCalled();
  },
);

it('retains uncertain native rotation bytes across selection changes and retries the same request', async () => {
  const mixed = {
    ...channel,
    definition: {
      ...channel.definition,
      sources: [source, { ...defaultSource('telegram'), bot_id: '321' }],
    },
  };
  const fetcher = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('Connection interrupted'))
    .mockImplementation(async () => response(mixed));
  vi.stubGlobal('fetch', fetcher);
  const view = (section: EditorSection) => (
    <RelayCapabilitiesContext value={available}>
      <ChannelEditor channel={mixed} section={section} onDone={vi.fn()} denied={vi.fn()} />
    </RelayCapabilitiesContext>
  );
  const { rerender } = render(view({ kind: 'producer', index: 1 }));
  enter('Telegram bot token', 'unsaved-telegram-private');
  rerender(view({ kind: 'producer', index: 0 }));
  enterBundle();
  fireEvent.click(screen.getByRole('button', { name: 'Save producer' }));
  await screen.findByRole('button', { name: 'Retry same request' });
  expect(screen.getByLabelText('Encrypt Key')).toHaveValue(bundle.encrypt_key);
  rerender(view({ kind: 'producer', index: 1 }));
  expect(screen.getByLabelText('Telegram bot token')).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Retry same request' }));
  await waitFor(() => expect(screen.getByLabelText('Telegram bot token')).toBeEnabled());
  expect(fetcher).toHaveBeenCalledTimes(2);
  expect(fetcher.mock.calls[1]![1].body).toBe(fetcher.mock.calls[0]![1].body);
  expect(fetcher.mock.calls[1]![1].headers).toEqual(fetcher.mock.calls[0]![1].headers);
  expect(screen.getByLabelText('Telegram bot token')).toHaveValue('unsaved-telegram-private');
  rerender(view({ kind: 'producer', index: 0 }));
  expect(screen.getByLabelText('Encrypt Key')).toHaveValue('');
});
