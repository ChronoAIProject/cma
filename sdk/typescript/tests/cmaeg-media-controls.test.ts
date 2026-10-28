import { describe, expect, it } from 'vitest';
import { NativeMediaCapabilitiesSchema } from '../src/cmaeg/types/messages.js';

describe('native media/control discovery', () => {
  const legacy = {
    kinds: ['image'],
    sources: ['base64'],
    max_attachments: 4,
    max_inline_bytes: 65536,
  };
  it('preserves the same-message bounds and explicit edit refusal', () => {
    const value = {
      ...legacy,
      controls: {
        max_attachments: 1,
        buttons: true,
        choices: true,
        caption_max_chars: 1024,
        update: false,
      },
    };
    expect(NativeMediaCapabilitiesSchema(value)).toEqual(value);
  });
  it('distinguishes a confirmed upload replacement from a preserving edit', () => {
    const value = {
      ...legacy,
      controls: {
        max_attachments: 1,
        buttons: true,
        choices: true,
        caption_max_chars: 1024,
        update: true,
        replace_media: true,
      },
    };
    expect(NativeMediaCapabilitiesSchema(value)).toEqual(value);
  });
  it('keeps legacy capability bytes without inventing combined delivery', () => {
    expect(NativeMediaCapabilitiesSchema(legacy)).toEqual(legacy);
  });
});
