import { expect, it } from 'vitest';
import { NativeMediaCapabilitiesSchema, OutboundMessageSchema } from '../src/cmaeg/types.js';

it('retains the bounded Discord voice discovery profile without changing generic audio', () => {
  const profile = {
    kind: 'voice',
    mime_types: ['audio/wav'],
    max_bytes: 3840044,
    max_attachments: 1,
    text: false,
    caption: false,
    profile: 'Canonical mono signed 16-bit little-endian PCM WAV at 16kHz, 0.1–120 seconds',
    message_operations: false,
  };
  const view = NativeMediaCapabilitiesSchema({
    kinds: ['image', 'file', 'audio', 'video', 'voice'],
    sources: ['url', 'base64'],
    max_attachments: 4,
    max_inline_bytes: 65536,
    presentation_profiles: [profile],
  });
  expect(view.presentation_profiles).toEqual([profile]);
  for (const kind of ['audio', 'voice']) {
    const attachment = { kind, source: { type: 'base64', data: 'AQIDBA==' } };
    expect(OutboundMessageSchema({ native_attachments: [attachment] }).native_attachments).toEqual([
      attachment,
    ]);
  }
});
