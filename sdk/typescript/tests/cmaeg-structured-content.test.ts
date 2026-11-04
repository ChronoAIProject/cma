import { expect, it } from 'vitest';
import {
  OutboundMessageSchema,
  CapabilitiesSchema,
  StructuredContentSchema,
} from '../src/cmaeg/types.js';

it('retains legacy messages and separately advertises optional static reply kinds', () => {
  expect(OutboundMessageSchema({ text: 'legacy' })).toEqual({ text: 'legacy' });
  expect(
    OutboundMessageSchema({
      structured_content: { type: 'contact', phone_number: '+6512345678', first_name: 'Alice' },
    }),
  ).toEqual({
    structured_content: { type: 'contact', phone_number: '+6512345678', first_name: 'Alice' },
  });
  expect(() => OutboundMessageSchema({ structured_content: null })).toThrow();
});

it('decodes finite coordinates and refuses nonfinite or unknown structured variants', () => {
  for (const content of [
    { type: 'location', latitude: -12.5, longitude: 103.8, horizontal_accuracy: 2.5 },
    { type: 'venue', latitude: 1.3, longitude: 103.8, title: 'Library', address: 'Main Street' },
  ])
    expect(StructuredContentSchema(content)).toEqual(content);
  for (const latitude of [NaN, Infinity, '1'])
    expect(() => StructuredContentSchema({ type: 'location', latitude, longitude: 0 })).toThrow();
  expect(() =>
    StructuredContentSchema({ type: 'live_location', latitude: 0, longitude: 0 }),
  ).toThrow();
});

it('preserves optional structured reply capability without inferring it on older gateways', () => {
  const legacy = {
    edit: false,
    typing: false,
    buttons: 'text',
    cards: 'text',
    attachments: 'none',
    max_text_chars: 4000,
  };
  expect(CapabilitiesSchema(legacy)).toEqual(legacy);
  const current = { ...legacy, structured_replies: ['contact', 'location', 'venue'] };
  expect(CapabilitiesSchema(current)).toEqual(current);
});
