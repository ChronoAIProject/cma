import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelEditor } from '../ChannelEditor';
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

import {
  available,
  identity,
  source,
  bundle,
  channel,
  enter,
  enterBundle,
  response,
} from './NativeLarkSourceFields.fixtures';
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
it('shows exact app identity and human admission without relay resources or bot controls', () => {
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
it('discovers native controls and forms independently and explains the callback subscription', () => {
  const view = (controls?: boolean, forms?: boolean) => (
    <RelayCapabilitiesContext
      value={{
        ...available,
        authoring: {
          ...available.authoring!,
          native_lark_controls_enabled: controls,
          native_lark_forms_enabled: forms,
        },
      }}
    >
      <SourceFields value={source} onChange={vi.fn()} />
    </RelayCapabilitiesContext>
  );
  const { rerender } = render(view());
  expect(
    screen.getByText(/Interactive card, button and choice replies are disabled/),
  ).toBeVisible();
  expect(screen.queryByText(/subscribe to card.action.trigger only/)).not.toBeInTheDocument();
  rerender(view(true, false));
  expect(screen.getByText(/subscribe to card.action.trigger only/)).toHaveTextContent(
    'This subscription is separate from message events.',
  );
  expect(screen.getByText(/Form replies are disabled/)).toBeVisible();
  rerender(view(true, true));
  expect(screen.getByText(/Form submissions are enabled for the original person/)).toBeVisible();
  rerender(view(false, false));
  expect(
    screen.getByText(/Interactive card, button and choice replies are disabled/),
  ).toBeVisible();
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
  expect(
    screen.queryByRole('menuitemradio', { name: /^Direct Lark \/ Feishu app/ }),
  ).not.toBeInTheDocument();
  fireEvent.keyDown(document, { key: 'Escape' });
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
