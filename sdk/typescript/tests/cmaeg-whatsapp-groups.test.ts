import { expect, it } from 'vitest';
import { SourceSchema, SourceRequestSchema } from '../src/cmaeg/types.js';
it('retains optional existing-group source opt-in without changing private source bytes', () => {
  const source = {
    type: 'whatsapp',
    app_id: '12345',
    waba_id: '23456',
    phone_number_id: '34567',
    admission: { type: 'all' },
  };
  for (const schema of [SourceSchema, SourceRequestSchema]) {
    expect(schema(source)).toEqual(source);
    const group = { ...source, group_ids: ['opaque+/=='] };
    expect(schema(group)).toEqual(group);
    expect(() => schema({ ...source, group_ids: [123] })).toThrow();
  }
});
