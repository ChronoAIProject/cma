import { expect, it } from 'vitest';
import { NativeDiscordActivationSchema } from '../src/cmaeg/types.js';

const legacy = { state: 'selected', detail: 'selected', capacity: 64, selected_records: 2 };
const connection = {
  scope: 'serving_instance',
  stage: 'unobserved',
  outcome: 'unknown',
  connected: null,
  authenticated: null,
  ready: null,
  lease: 'unknown',
  identify: 'not_attempted',
  stale: false,
  last_dispatch_outcome: 'unknown',
};
it('keeps selection separate from optional unknown, healthy and failed connection observations', () => {
  expect(NativeDiscordActivationSchema(legacy)).toEqual(legacy);
  for (const status of [
    connection,
    {
      ...connection,
      stage: 'ready',
      outcome: 'ok',
      connected: true,
      authenticated: true,
      ready: true,
    },
    {
      ...connection,
      stage: 'lease',
      outcome: 'retrying',
      connected: false,
      last_failure_stage: 'lease',
      last_failure_code: 'lease_contended',
    },
    { ...connection, stale: true },
  ]) {
    const value = { ...legacy, connection: status };
    expect(NativeDiscordActivationSchema(value)).toEqual(value);
  }
  expect(() =>
    NativeDiscordActivationSchema({ ...legacy, connection: { ...connection, connected: 'true' } }),
  ).toThrow();
});
