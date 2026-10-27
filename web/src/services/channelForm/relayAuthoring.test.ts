import { expect, it } from 'vitest';
import type { RelayDiscovery } from '../RelayCapabilities';
import { validateRelayAuthoring } from './relayAuthoring';
const enabled: RelayDiscovery = {
  status: 'ready',
  authoring: {
    dynamic_sources_writes_enabled: false,
    relay_raw_verification_writes_enabled: true,
    multi_consumer_writes_enabled: false,
    native_bot_mentions_writes_enabled: false,
    thread_contract_writes_enabled: false,
    platforms: [
      {
        id: 'discord',
        label: 'Discord',
        verification_modes: [
          { id: 'raw_event', writes_enabled: true },
          { id: 'trusted_normalized', writes_enabled: true },
        ],
      },
    ],
  },
};
const raw = {
  name: 'Channel',
  sources: [
    {
      type: 'nyxid_relay',
      platform: 'discord',
      verify_raw_event: true,
      route_ids: ['owner-route'],
    },
  ],
  partition: { key: 'conversation' },
};
it('requires affirmative writer support for new raw opt-ins, retaining the draft', () => {
  for (const status of ['loading', 'legacy', 'failed'] as const)
    expect(() => validateRelayAuthoring({ status }, raw)).toThrow(/support|Checking/);
  expect(raw.sources[0]?.verify_raw_event).toBe(true);
  expect(() => validateRelayAuthoring(enabled, raw)).not.toThrow();
});
it('preserves exact saved raw settings for an unrelated edit on an older gateway', () => {
  expect(() =>
    validateRelayAuthoring({ status: 'legacy' }, { ...raw, name: 'Renamed' }, raw),
  ).not.toThrow();
  expect(() =>
    validateRelayAuthoring(
      { status: 'legacy' },
      { ...raw, sources: [{ ...raw.sources[0], route_ids: ['different'] }] },
      raw,
    ),
  ).toThrow(/unchanged/);
  expect(() =>
    validateRelayAuthoring({ status: 'legacy' }, raw, raw, { channel_key: 'replacement' }),
  ).toThrow(/unchanged/);
  expect(() =>
    validateRelayAuthoring({ status: 'legacy' }, raw, raw, { agent_key: 'provider-replacement' }),
  ).not.toThrow();
});
it('never substitutes normalized trust when the verification choice is missing', () => {
  const draft = { sources: [{ type: 'nyxid_relay', platform: 'discord' }] };
  expect(() => validateRelayAuthoring(enabled, draft)).toThrow(/explicit incoming/);
  expect(draft.sources[0]).not.toHaveProperty('trust_normalized');
});
it('allows explicitly selected legacy trust while preventing a new unknown mode', () => {
  expect(() =>
    validateRelayAuthoring(
      { status: 'legacy' },
      { sources: [{ type: 'nyxid_relay', platform: 'discord', trust_normalized: true }] },
    ),
  ).not.toThrow();
  expect(() =>
    validateRelayAuthoring(enabled, {
      sources: [{ type: 'nyxid_relay', platform: 'whatsapp', verify_raw_event: true }],
    }),
  ).toThrow(/does not advertise/);
});

it('does not treat empty credential fields or a cancelled HMAC change as a source mutation', () => {
  expect(() =>
    validateRelayAuthoring({ status: 'legacy' }, raw, raw, {
      channel_key: '',
      replace_hmac: false,
      hmac_secrets: [{ key_id: 'draft', secret: 'discarded' }],
    }),
  ).not.toThrow();
});
