import { describe, expect, it } from 'vitest';
import { quantity } from '../src/usage-metrics/schema.js';
import { ProtocolError } from '../src/errors.js';

describe('measured quantities preserve source precision', () => {
  it('preserves large decimal values and fractional observations', () => {
    expect(quantity('18446744073709551615')).toBe('18446744073709551615');
    expect(quantity('9007199254740992')).toBe('9007199254740992');
    expect(quantity(0.003737125)).toBe(0.003737125);
    expect(quantity(9007199254740991)).toBe(9007199254740991);
  });
  it.each([
    Number.NaN,
    Number.POSITIVE_INFINITY,
    -1,
    9007199254740992,
    '0123',
    '100',
    '-1',
    '1e30',
    null,
  ])('rejects ambiguous quantity %s', (value) => {
    expect(() => quantity(value)).toThrow(ProtocolError);
  });
});
