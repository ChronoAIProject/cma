import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelEditor, type Channel } from '../ChannelEditor';
import { ChannelEndpoints } from '../ChannelEndpoints';
import {
  RelayCapabilitiesContext,
  nativeWhatsappUnavailable,
  type RelayDiscovery,
} from '../RelayCapabilities';
import { NodeFields, nodeCredentials } from './NodeFields';
import { SourceFields } from './SourceFields';
import { channelRequest, credentialsPatch, defaultSource } from './model';
import { validateRelayAuthoring } from './relayAuthoring';

const available: RelayDiscovery = {
  status: 'ready',
  authoring: {
    native_whatsapp_writes_enabled: true,
    native_whatsapp_auto_activation_enabled: true,
    multi_consumer_writes_enabled: true,
    dynamic_sources_writes_enabled: false,
    relay_raw_verification_writes_enabled: false,
    native_bot_mentions_writes_enabled: false,
    thread_contract_writes_enabled: false,
    platforms: [],
  },
};
const source = {
  ...defaultSource('whatsapp'),
  app_id: '123',
  waba_id: '456',
  phone_number_id: '789',
};
const bundle = {
  app_secret: 'sample-app-private-value',
  access_token: 'sample-access-private-value',
  verify_token: 'sample-webhook-private-value',
};
const channel: Channel = {
  channel_id: 'owned-whatsapp',
  record_id: '11111111-1111-4111-8111-111111111111',
  version: 2,
  provider: { slug: 'cma_codex', level: 'l3' },
  definition: { name: 'Direct WhatsApp', sources: [source] },
  endpoints: {
    custom_event_url: '',
    nyxid_callback_url: '',
    whatsapp_webhook_url: 'https://example.test/whatsapp/owned',
  },
};
afterEach(() => vi.unstubAllGlobals());

it('requires explicit delivery support and a complete credential bundle for a new source', () => {
  expect(nativeWhatsappUnavailable({ status: 'legacy' })).toContain('not enabled');
  const disabled = {
    ...available,
    authoring: { ...available.authoring!, native_whatsapp_auto_activation_enabled: false },
  };
  expect(() =>
    validateRelayAuthoring(disabled, { sources: [source] }, undefined, { whatsapp: bundle }),
  ).toThrow('message delivery');
  expect(() => validateRelayAuthoring(available, { sources: [source] })).toThrow('all three');
  expect(() =>
    validateRelayAuthoring(available, { sources: [source] }, undefined, { whatsapp: bundle }),
  ).not.toThrow();
  expect(() =>
    validateRelayAuthoring(
      available,
      { sources: [source], partition: { key: 'thread' } },
      undefined,
      { whatsapp: bundle },
    ),
  ).toThrow('private chats');
});

it('preserves saved sources with no secret replacement when authoring is disabled', () => {
  expect(() =>
    validateRelayAuthoring(
      { status: 'legacy' },
      { sources: [source] },
      { sources: [source] },
      { whatsapp: {} },
    ),
  ).not.toThrow();
  expect(
    credentialsPatch({ whatsapp: { app_secret: '', access_token: '', verify_token: '' } }, false),
  ).toBeUndefined();
  expect(() =>
    validateRelayAuthoring(
      { status: 'legacy' },
      { sources: [source] },
      { sources: [source] },
      { whatsapp: bundle },
    ),
  ).toThrow('not enabled');
  expect(() =>
    credentialsPatch({ whatsapp: { access_token: bundle.access_token } }, false),
  ).toThrow('all three');
});

it('drops stale native credentials after switching producer type and excludes them from consumer saves', () => {
  expect(
    nodeCredentials(
      { whatsapp: bundle, agent_key: 'provider-private' },
      { kind: 'consumer', id: 'default' },
      { sources: [source] },
    ),
  ).toEqual({ agent_key: 'provider-private' });
  expect(
    channelRequest(
      { sources: [{ ...defaultSource('custom'), auth: { type: 'nyxid_key', key_hashes: [] } }] },
      { agent_key: 'provider-private', whatsapp: bundle },
      undefined,
      channel.record_id,
    ),
  ).toMatchObject({ credentials: { agent_key: 'provider-private' } });
  expect(
    JSON.stringify(
      channelRequest(
        { sources: [{ ...defaultSource('custom'), auth: { type: 'nyxid_key', key_hashes: [] } }] },
        { agent_key: 'provider-private', whatsapp: bundle },
        undefined,
        channel.record_id,
      ),
    ),
  ).not.toContain(bundle.access_token);
  expect(
    channelRequest(
      { sources: [source] },
      { agent_key: 'provider-private', whatsapp: bundle },
      undefined,
      channel.record_id,
    ),
  ).toMatchObject({ credentials: { whatsapp: bundle } });
});

it('uses dedicated account fields and private chat admission without relay or group controls', () => {
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
            groups: 'deny',
          },
        }}
        onChange={onChange}
      />
    </RelayCapabilitiesContext>,
  );
  expect(screen.getByLabelText('Meta app ID')).toHaveValue('123');
  expect(screen.queryByLabelText('NyxID issuer')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Group messages' })).not.toBeInTheDocument();
  fireEvent.change(screen.getByLabelText('Phone number ID'), { target: { value: '901' } });
  expect(onChange).toHaveBeenCalledWith(
    expect.objectContaining({ phone_number_id: '901', waba_id: '456' }),
  );
});

it('places masked Meta credentials with their producer and omits unrelated platform credentials', () => {
  render(
    <RelayCapabilitiesContext value={available}>
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
    </RelayCapabilitiesContext>,
  );
  for (const name of ['Meta app secret', 'Meta access token', 'Webhook verification token']) {
    expect(screen.getByLabelText(name)).toHaveAttribute('type', 'password');
    expect(screen.getByLabelText(name)).toHaveValue('');
  }
  expect(screen.queryByLabelText('Telegram bot token')).not.toBeInTheDocument();
  expect(screen.queryByLabelText('NyxID channel key')).not.toBeInTheDocument();
  expect(screen.queryByLabelText('Replace HMAC credentials')).not.toBeInTheDocument();
});

it('sends an atomic rotation once and clears all private fields after confirmed save', async () => {
  const fetcher = vi
    .fn()
    .mockResolvedValue(new Response(JSON.stringify({ ...channel, version: 3 })));
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
  for (const [label, value] of [
    ['Meta app secret', bundle.app_secret],
    ['Meta access token', bundle.access_token],
    ['Webhook verification token', bundle.verify_token],
  ])
    fireEvent.change(screen.getByLabelText(label!), { target: { value } });
  fireEvent.click(screen.getByRole('button', { name: 'Save producer' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(1));
  expect(JSON.parse(fetcher.mock.calls[0]![1].body)).toMatchObject({
    expected_version: 2,
    credentials: { whatsapp: bundle },
  });
  await waitFor(() => expect(screen.getByLabelText('Meta app secret')).toHaveValue(''));
  expect(screen.getByLabelText('Meta access token')).toHaveValue('');
  expect(screen.getByLabelText('Webhook verification token')).toHaveValue('');
});

it('shows the Meta callback with the existing endpoint copy control', () => {
  render(
    <ChannelEndpoints
      endpoints={{ whatsapp_webhook_url: channel.endpoints.whatsapp_webhook_url }}
    />,
  );
  expect(screen.getByText('WhatsApp webhook')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Copy WhatsApp webhook' })).toBeInTheDocument();
  expect(screen.queryByText('Callback endpoint')).not.toBeInTheDocument();
});

it('requires separate group authoring support and preserves removal while all authoring is disabled', () => {
  const grouped = { ...source, group_ids: ['opaque+/=='] };
  expect(() =>
    validateRelayAuthoring(available, { sources: [grouped] }, { sources: [source] }),
  ).toThrow('group opt-ins');
  const groups = {
    ...available,
    authoring: { ...available.authoring!, native_whatsapp_groups_writes_enabled: true },
  };
  expect(() =>
    validateRelayAuthoring(groups, { sources: [grouped] }, { sources: [source] }),
  ).not.toThrow();
  expect(() =>
    validateRelayAuthoring({ status: 'legacy' }, { sources: [grouped] }, { sources: [grouped] }),
  ).not.toThrow();
  expect(() =>
    validateRelayAuthoring({ status: 'legacy' }, { sources: [source] }, { sources: [grouped] }),
  ).not.toThrow();
  for (const group_ids of [
    ['a', 'a'],
    ['a'.repeat(513)],
    ['../escape'],
    Array.from({ length: 17 }, (_, i) => `group${i}`),
  ]) {
    expect(() =>
      validateRelayAuthoring(
        groups,
        { sources: [{ ...source, group_ids }] },
        { sources: [source] },
      ),
    ).toThrow('16 unique');
  }
});

it('shows structured existing-group opt-in with only supported group admission rules', () => {
  const onChange = vi.fn();
  render(
    <RelayCapabilitiesContext
      value={{
        ...available,
        authoring: { ...available.authoring!, native_whatsapp_groups_writes_enabled: true },
      }}
    >
      <SourceFields
        value={{
          ...source,
          group_ids: ['opaque+/=='],
          admission: {
            type: 'scoped',
            senders: { type: 'open' },
            chats: { type: 'open' },
            groups: 'all',
          },
        }}
        onChange={onChange}
      />
    </RelayCapabilitiesContext>,
  );
  expect(screen.getByLabelText('Allowed group IDs 1')).toHaveValue('opaque+/==');
  expect(screen.getByRole('button', { name: 'Group messages' })).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Remove Allowed group IDs 1' }));
  expect(onChange).toHaveBeenCalledWith(
    expect.not.objectContaining({ group_ids: expect.anything() }),
  );
  expect(screen.getByText(/Official Business Account/)).toBeInTheDocument();
});

it('keeps managed form consent separate from operator availability and permits withdrawal', () => {
  const consenting = { ...source, forms_enabled: true };
  expect(() =>
    validateRelayAuthoring(available, { sources: [consenting] }, { sources: [source] }),
  ).toThrow('form consent');
  expect(() =>
    validateRelayAuthoring(available, { sources: [source] }, { sources: [consenting] }),
  ).not.toThrow();
  const enabled = {
    ...available,
    authoring: {
      ...available.authoring!,
      native_whatsapp_forms_enabled: true,
      native_whatsapp_flow_writes_enabled: true,
    },
  };
  expect(() =>
    validateRelayAuthoring(enabled, { sources: [consenting] }, { sources: [source] }),
  ).not.toThrow();
  const change = vi.fn();
  render(
    <RelayCapabilitiesContext value={available}>
      <SourceFields value={consenting} onChange={change} />
    </RelayCapabilitiesContext>,
  );
  const checkbox = screen.getByLabelText(/Allow managed WhatsApp forms/);
  expect(checkbox).toBeEnabled();
  fireEvent.click(checkbox);
  expect(change).toHaveBeenCalledWith(expect.objectContaining({ forms_enabled: false }));
});

it('gates coexistence incoming changes independently and allows saved consent withdrawal', () => {
  const consenting = { ...source, message_events: true };
  expect(() =>
    validateRelayAuthoring(available, { sources: [consenting] }, { sources: [source] }),
  ).toThrow('incoming message change consent');
  const enabled = {
    ...available,
    authoring: { ...available.authoring!, native_whatsapp_message_events_enabled: true },
  };
  expect(() =>
    validateRelayAuthoring(enabled, { sources: [consenting] }, { sources: [source] }),
  ).not.toThrow();
  expect(() =>
    validateRelayAuthoring(available, { sources: [source] }, { sources: [consenting] }),
  ).not.toThrow();
  const change = vi.fn();
  render(
    <RelayCapabilitiesContext value={available}>
      <SourceFields value={consenting} onChange={change} />
    </RelayCapabilitiesContext>,
  );
  const checkbox = screen.getByLabelText(/Allow coexistence incoming edits and deletions/);
  expect(checkbox).toBeEnabled();
  fireEvent.click(checkbox);
  expect(change).toHaveBeenCalledWith(expect.objectContaining({ message_events: false }));
  expect(screen.getByText(/Business App coexistence accounts/)).toBeInTheDocument();
});
