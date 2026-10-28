import { expect, it } from 'vitest';
import { ActivityKindSchema, QuotedMessageSchema } from '../src/cmaeg/types.js';

it('reads quoted content and native references without changing legacy activity shape', () => {
  const old = { type: 'message', text: 'current', mentions_bot: false };
  expect(ActivityKindSchema(old)).toEqual(old);
  for (const quote of [
    {
      relation: 'reply',
      message_id: '8',
      content: {
        text: { text: 'untrusted quote', truncated: false, origin: 'selected_quote' },
        kinds: ['image'],
      },
    },
    { relation: 'context', message_id: 'wamid.parent' },
  ]) {
    expect(QuotedMessageSchema(quote)).toEqual(quote);
    expect(ActivityKindSchema({ ...old, quoted_message: quote })).toEqual({
      ...old,
      quoted_message: quote,
    });
  }
  expect(
    QuotedMessageSchema({ relation: 'reply', message_id: '8', url: 'https://private.invalid' }),
  ).toEqual({ relation: 'reply', message_id: '8' });
  expect(() => QuotedMessageSchema({ relation: 'forward', message_id: '8' })).toThrow();
});
