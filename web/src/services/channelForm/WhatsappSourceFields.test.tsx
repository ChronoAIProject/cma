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
    ),
  ).toEqual({ agent_key: 'provider-private' });
  expect(
    channelRequest(
      { sources: [defaultSource('custom')] },
      { agent_key: 'provider-private', whatsapp: bundle },
      undefined,
      channel.record_id,
    ),
  ).toMatchObject({ credentials: { agent_key: 'provider-private' } });
  expect(
    JSON.stringify(
      channelRequest(
        { sources: [defaultSource('custom')] },
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
