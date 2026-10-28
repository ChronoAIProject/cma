import { describe, expect, it } from 'vitest';
import { IntentReceiptSchema } from '../src/cmaeg/types.js';

describe('native split-text receipts', () => {
  const rendering = { outcome: 'degraded', blocks: [] };
  it('preserves omitted legacy fields and every confirmed part on unknown delivery', () => {
    const legacy = { rendering, delivery: { outcome: 'confirmed' }, message_ref: 'legacy' };
    expect(IntentReceiptSchema(legacy)).toEqual(legacy);
    const partial = {
      rendering,
      delivery: { outcome: 'unknown', reason: 'partial_delivery' },
      message_parts: [
        { index: 0, message_ref: 'confirmed-first' },
        { index: 1, message_ref: 'confirmed-second' },
      ],
    };
    expect(IntentReceiptSchema(partial)).toEqual(partial);
    expect(IntentReceiptSchema(JSON.parse(JSON.stringify(partial)))).toEqual(partial);
  });
  it('refuses a malformed part rather than dropping its authority', () => {
    expect(() =>
      IntentReceiptSchema({
        rendering,
        delivery: { outcome: 'confirmed' },
        message_parts: [{ index: 0 }],
      }),
    ).toThrow();
    expect(() =>
      IntentReceiptSchema({ rendering, delivery: { outcome: 'confirmed' }, message_parts: null }),
    ).toThrow();
  });
});
