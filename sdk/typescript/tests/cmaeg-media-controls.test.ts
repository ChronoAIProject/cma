import { describe, expect, it } from 'vitest';
import {
  NativeMediaCapabilitiesSchema,
  OutboundMessageSchema,
} from '../src/cmaeg/types/messages.js';

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
  it('preserves album kind groups and explicit operation refusal', () => {
    const value = {
      ...legacy,
      albums: {
        min_attachments: 2,
        max_attachments: 4,
        kind_groups: [['image', 'video'], ['file'], ['audio']],
        controls: false,
        message_operations: false,
      },
    };
    expect(NativeMediaCapabilitiesSchema(value)).toEqual(value);
  });
  it('keeps explicit Lark replacement authority separate from new-card issuance count', () => {
    const capabilities = {
      ...legacy,
      controls: {
        kinds: ['image'],
        max_attachments: 1,
        buttons: true,
        choices: true,
        caption_max_chars: 0,
        update: true,
        replace_media: true,
      },
    };
    expect(NativeMediaCapabilitiesSchema(capabilities)).toEqual(capabilities);
    for (const count of [1, 4]) {
      const request = {
        text: 'Replacement images',
        blocks: [{ type: 'buttons', id: 'approval', options: [{ id: 'yes', label: 'Yes' }] }],
        native_attachments: Array.from({ length: count }, (_, index) => ({
          kind: 'image',
          source: { type: 'base64', data: 'cG5n' },
          filename: `replacement-${index}.png`,
          mime_type: 'image/png',
        })),
      };
      expect(OutboundMessageSchema(request)).toEqual(request);
    }
  });
  it('discovers complete Discord sets and explicit clearing without changing legacy empties', () => {
    const value = {
      ...legacy,
      controls: {
        max_attachments: 4,
        buttons: true,
        choices: true,
        caption_max_chars: 1024,
        update: true,
        replace_media: true,
        clear_media: true,
      },
    };
    expect(NativeMediaCapabilitiesSchema(value)).toEqual(value);
    for (const message of [
      { text: 'Keep files', native_attachments: [] },
      { text: 'Keep files', clear_native_attachments: false },
      { text: 'Clear files', clear_native_attachments: true },
    ]) {
      expect(OutboundMessageSchema(message)).toEqual(message);
    }
    expect(() => OutboundMessageSchema({ text: 'Clear', clear_native_attachments: null })).toThrow(
      'clear_native_attachments',
    );
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
