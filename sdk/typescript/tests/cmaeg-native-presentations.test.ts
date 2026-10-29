import { expect, it } from 'vitest';
import { NativeMediaCapabilitiesSchema, OutboundMessageSchema } from '../src/cmaeg/types.js';

it('preserves generic media bytes and accepts explicit native presentation kinds', () => {
  for (const kind of ['audio', 'video', 'voice', 'animation', 'sticker']) {
    const message = {
      native_attachments: [{ kind, source: { type: 'base64', data: 'AQIDBA==' } }],
    };
    expect(OutboundMessageSchema(message)).toEqual(message);
  }
});

it('keeps legacy discovery unchanged and parses exact optional presentation profiles', () => {
  const legacy = {
    kinds: ['audio', 'video'],
    sources: ['base64'],
    max_attachments: 4,
    max_inline_bytes: 65536,
  };
  expect(NativeMediaCapabilitiesSchema(legacy)).toEqual(legacy);
  const advertised = {
    ...legacy,
    kinds: ['audio', 'video', 'voice'],
    presentation_profiles: [
      {
        kind: 'voice',
        mime_types: ['audio/ogg'],
        max_bytes: 8388608,
        caption: false,
        profile: 'One complete mono Opus Ogg stream; no transcoding',
        message_operations: false,
      },
    ],
  };
  expect(NativeMediaCapabilitiesSchema(advertised)).toEqual(advertised);
});
