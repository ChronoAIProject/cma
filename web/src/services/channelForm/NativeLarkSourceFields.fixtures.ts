import { fireEvent, screen } from '@testing-library/react';
import type { Channel } from '../ChannelEditor';
import type { RelayDiscovery } from '../RelayCapabilities';
import { defaultSource } from './model';

export const available: RelayDiscovery = {
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
export const identity = {
  region: 'feishu',
  app_id: 'cli_separate_app',
  tenant_key: 'tenant_one',
  bot_open_id: 'ou_separate_bot',
};
export const source = {
  ...defaultSource('native_lark'),
  separate_app: true,
  identity,
  admission: { type: 'allowlist', sender_ids: ['ou_allowed_human'] },
};
export const bundle = {
  app_secret: 'sample-app-private-value',
  encrypt_key: 'sample-encrypt-private-value',
  verification_token: 'sample-verify-private-value',
};
export const channel: Channel = {
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
export const enter = (name: string, value: string) =>
  fireEvent.change(screen.getByLabelText(name), { target: { value } });
export const enterBundle = () => {
  enter('App secret', bundle.app_secret);
  enter('Encrypt Key', bundle.encrypt_key);
  enter('Verification Token', bundle.verification_token);
};
export const response = (value = channel) => new Response(JSON.stringify({ ...value, version: 3 }));
