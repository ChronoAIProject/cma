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
  it('preserves buttons-only header kinds without broadening the parent media list', () => {
    const value = {
      ...legacy,
      kinds: ['image', 'file', 'audio', 'sticker', 'video'],
      controls: {
        kinds: ['image', 'file', 'video'],
        max_attachments: 1,
        buttons: true,
        choices: false,
        caption_max_chars: 0,
        update: false,
      },
    };
    expect(NativeMediaCapabilitiesSchema(value)).toEqual(value);
  });
  it('preserves a four-image Lark control limit without replacement authority', () => {
    const value = {
      ...legacy,
      controls: {
        kinds: ['image'],
        max_attachments: 4,
        buttons: true,
        choices: true,
        caption_max_chars: 0,
        update: true,
      },
    };
    expect(NativeMediaCapabilitiesSchema(value)).toEqual(value);
  });
  it('keeps legacy capability bytes without inventing combined delivery', () => {
    expect(NativeMediaCapabilitiesSchema(legacy)).toEqual(legacy);
  });
  it('preserves the Lark image-only combination without promising file controls', () => {
    const value = {
      ...legacy,
      kinds: ['image', 'file', 'audio', 'video'],
      controls: {
        kinds: ['image'],
        max_attachments: 1,
        buttons: true,
        choices: true,
        caption_max_chars: 0,
        update: true,
      },
    };
    expect(NativeMediaCapabilitiesSchema(value)).toEqual(value);
  });
});
