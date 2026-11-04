import { describe, expect, it } from 'vitest';
import { CapabilitiesSchema } from '../src/cmaeg/types/activities.js';

describe('native Lark thread-start discovery', () => {
  const legacy = {
    attachments: 'link_text',
    buttons: 'numbered_text',
    cards: 'text',
    edit: false,
    max_text_chars: 4000,
    typing: false,
  };

  it('preserves an older server without creating thread-start authority', () => {
    expect(CapabilitiesSchema(legacy)).toEqual(legacy);
  });

  it.each([true, false])('preserves the explicit native thread-start value %s', (enabled) => {
    const value = { ...legacy, native_thread_start: enabled };
    expect(CapabilitiesSchema(value)).toEqual(value);
    expect(CapabilitiesSchema(value)).not.toHaveProperty('thread_lifecycle');
  });
});
