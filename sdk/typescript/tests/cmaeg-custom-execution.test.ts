import { readFileSync } from 'node:fs';
import { expect, it } from 'vitest';
import {
  ChannelCreateRequestSchema,
  ChannelUpdateRequestSchema,
  PublicChannelDefinitionSchema,
  RelayAuthoringCapabilitiesSchema,
} from '../src/cmaeg/types.js';

const examples = JSON.parse(
  readFileSync(
    new URL('../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json', import.meta.url),
    'utf8',
  ),
) as Record<string, Record<string, unknown>[]>;

it('preserves absent legacy fields and validates an additive create choice, saved marker and discovery', () => {
  for (const [name, schema, field] of [
    ['ChannelCreateRequest', ChannelCreateRequestSchema, 'durable_custom_execution'],
    ['PublicChannelDefinition', PublicChannelDefinitionSchema, 'durable_custom_execution'],
    [
      'RelayAuthoringCapabilities',
      RelayAuthoringCapabilitiesSchema,
      'custom_durable_execution_enabled',
    ],
  ] as const) {
    const legacy = { ...examples[name]![0] };
    delete legacy[field];
    expect(schema(legacy)).not.toHaveProperty(field);
    for (const enabled of [false, true])
      expect(schema({ ...legacy, [field]: enabled })).toHaveProperty(field, enabled);
    expect(() => schema({ ...legacy, [field]: 'true' })).toThrow();
  }
});

it('keeps the execution choice out of ordinary update serialization', () => {
  const update = examples.ChannelUpdateRequest![0]!;
  expect(ChannelUpdateRequestSchema({ ...update, durable_custom_execution: true })).toEqual(
    ChannelUpdateRequestSchema(update),
  );
});
