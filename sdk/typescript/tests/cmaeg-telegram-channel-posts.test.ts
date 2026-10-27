import { expect, it } from 'vitest';
import { SourceSchema, SourceRequestSchema } from '../src/cmaeg/types.js';

it('preserves omitted channel posts and the independent explicit native source opt-in', () => {
  const legacy = { type: 'telegram', bot_id: '77', admission: { type: 'all' } };
  for (const value of [legacy, { ...legacy, message_events: true, channel_posts: true }]) {
    expect(SourceSchema(value)).toEqual(value);
    expect(SourceRequestSchema(value)).toEqual(value);
  }
  expect(() => SourceRequestSchema({ ...legacy, channel_posts: 'true' })).toThrow();
});
