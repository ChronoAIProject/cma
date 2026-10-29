import { expect, it } from 'vitest';
import { CapabilityViewSchema, RelayAuthoringCapabilitiesSchema } from '../src/cmaeg/types.js';

const legacy = {
  dynamic_sources_writes_enabled: true,
  relay_raw_verification_writes_enabled: true,
  multi_consumer_writes_enabled: true,
  native_bot_mentions_writes_enabled: false,
  thread_contract_writes_enabled: false,
  native_discord_writes_enabled: true,
  native_discord_reactions_enabled: true,
  platforms: [],
};

it('keeps legacy Discord discovery unchanged without inferring forward snapshot support', () => {
  expect(RelayAuthoringCapabilitiesSchema(legacy)).toEqual(legacy);
  const view = { platforms: [], relay_authoring: legacy };
  expect(CapabilityViewSchema(view)).toEqual(view);
});

it('preserves explicit forward snapshot discovery and rejects malformed values', () => {
  for (const enabled of [false, true, null]) {
    const value = { ...legacy, native_discord_forwards_enabled: enabled };
    expect(RelayAuthoringCapabilitiesSchema(value)).toEqual(value);
    const view = { platforms: [], relay_authoring: value };
    expect(CapabilityViewSchema(view)).toEqual(view);
  }
  for (const enabled of ['true', 1, {}, []]) {
    expect(() =>
      RelayAuthoringCapabilitiesSchema({
        ...legacy,
        native_discord_forwards_enabled: enabled,
      }),
    ).toThrow();
  }
});
