import { expect, it } from 'vitest';
import {
  SourceSchema,
  SourceRequestSchema,
  SourceKindSchema,
  NativeLarkCredentialsSchema,
  RelayAuthoringCapabilitiesSchema,
} from '../src/cmaeg/types.js';

it('keeps native app, regional identity and secrets distinct from relay authority', () => {
  for (const region of ['lark', 'feishu']) {
    const identity = {
      region,
      app_id: 'cli_separate_app',
      tenant_key: 'native_tenant',
      bot_open_id: 'ou_native_bot',
    };
    const source = {
      type: 'native_lark',
      separate_app: true,
      identity,
      admission: { type: 'all' },
    };
    expect(SourceSchema(source)).toEqual(source);
    expect(SourceRequestSchema(source)).toEqual(source);
    const activity = { type: 'native_lark', ...identity };
    expect(SourceKindSchema(activity)).toEqual(activity);
  }
  const credentials = {
    app_secret: 'fixture-secret',
    encrypt_key: 'fixture-encrypt',
    verification_token: 'fixture-token',
  };
  expect(NativeLarkCredentialsSchema(credentials)).toEqual(credentials);
  expect(() => NativeLarkCredentialsSchema({ app_secret: 'incomplete' })).toThrow();
});

it('does not infer native Lark availability from relay gates', () => {
  const legacy = {
    dynamic_sources_writes_enabled: true,
    relay_raw_verification_writes_enabled: true,
    multi_consumer_writes_enabled: true,
    native_bot_mentions_writes_enabled: false,
    thread_contract_writes_enabled: false,
    platforms: [],
  };
  expect(RelayAuthoringCapabilitiesSchema(legacy)).toEqual(legacy);
  const current = {
    ...legacy,
    native_lark_writes_enabled: false,
    native_lark_intake_enabled: false,
  };
  expect(RelayAuthoringCapabilitiesSchema(current)).toEqual(current);
});
