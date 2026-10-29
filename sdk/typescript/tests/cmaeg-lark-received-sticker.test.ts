import { describe, expect, it } from 'vitest';
import { CapabilitiesSchema } from '../src/cmaeg/types/activities.js';
import { OutboundMessageSchema } from '../src/cmaeg/types/messages.js';

describe('received Lark sticker compatibility', () => {
  it('preserves legacy messages without creating sticker authority', () => {
    const message = { text: 'unchanged', blocks: [] };
    expect(OutboundMessageSchema(message)).toEqual(message);
    expect(OutboundMessageSchema(message)).not.toHaveProperty('reply_with_received_sticker');
  });
  it.each([true, false])('preserves explicit resend request %s', (value) => {
    const message = { reply_with_received_sticker: value, blocks: [] };
    expect(OutboundMessageSchema(message)).toEqual(message);
  });
  it('rejects null or nonboolean requests and never projects a caller-supplied key', () => {
    for (const value of [null, 'true', 1, { file_key: 'unsafe' }]) {
      expect(() => OutboundMessageSchema({ reply_with_received_sticker: value })).toThrow();
    }
    expect(
      OutboundMessageSchema({ reply_with_received_sticker: true, file_key: 'unsafe' }),
    ).not.toHaveProperty('file_key');
  });
  it('preserves capability omission and explicit availability', () => {
    const legacy = {
      attachments: 'link_text',
      buttons: 'numbered_text',
      cards: 'text',
      edit: false,
      max_text_chars: 6000,
      typing: false,
    };
    expect(CapabilitiesSchema(legacy)).toEqual(legacy);
    const enabled = { ...legacy, reply_with_received_sticker: true };
    expect(CapabilitiesSchema(enabled)).toEqual(enabled);
    expect(CapabilitiesSchema(enabled)).not.toHaveProperty('native_media');
  });
});
