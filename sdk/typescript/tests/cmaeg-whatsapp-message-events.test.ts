import { expect, it } from 'vitest';
import {
  SourceSchema,
  SourceRequestSchema,
  RelayAuthoringCapabilitiesSchema,
} from '../src/cmaeg/types.js';

it('preserves omitted source consent and accepts independent coexistence consent', () => {
  const legacy = {
    type: 'whatsapp',
    app_id: '12345',
    waba_id: '23456',
    phone_number_id: '34567',
    admission: { type: 'all' },
  };
  for (const schema of [SourceSchema, SourceRequestSchema]) {
    expect(schema(legacy)).toEqual(legacy);
    for (const message_events of [true, false]) {
      expect(schema({ ...legacy, message_events })).toEqual({ ...legacy, message_events });
    }
    expect(() => schema({ ...legacy, message_events: 'true' })).toThrow();
  }
});
it('does not infer incoming message change support from ordinary native authoring', () => {
  const legacy = {
    dynamic_sources_writes_enabled: true,
    relay_raw_verification_writes_enabled: true,
    multi_consumer_writes_enabled: true,
    native_bot_mentions_writes_enabled: false,
    thread_contract_writes_enabled: false,
    platforms: [],
    native_whatsapp_writes_enabled: true,
  };
  expect(
    RelayAuthoringCapabilitiesSchema(legacy).native_whatsapp_message_events_enabled,
  ).toBeUndefined();
  for (const native_whatsapp_message_events_enabled of [true, false]) {
    const value = { ...legacy, native_whatsapp_message_events_enabled };
    expect(RelayAuthoringCapabilitiesSchema(value)).toEqual(value);
  }
});
