import { describe, expect, it } from 'vitest';
import { ProviderBindingSchema } from '../src/cmaeg/types.js';

describe('retained provider capabilities', () => {
  const legacy = { binding_id: 'bnd_synthetic', level: 'l1', slug: 'nyxbot' };

  it('preserves legacy absence and an acknowledged optional capability', () => {
    expect(ProviderBindingSchema(legacy)).toEqual(legacy);
    const negotiated = { ...legacy, capabilities: ['nyxid_turn_trace_v1'] };
    expect(ProviderBindingSchema(negotiated)).toEqual(negotiated);
  });

  it('rejects malformed capability arrays', () => {
    for (const capabilities of ['nyxid_turn_trace_v1', [42], {}, null]) {
      expect(() => ProviderBindingSchema({ ...legacy, capabilities })).toThrow();
    }
  });
});
