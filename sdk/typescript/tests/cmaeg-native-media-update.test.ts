import { readFileSync } from 'node:fs';
import { expect, it, vi } from 'vitest';
import { Cmaeg } from '../src/cmaeg/index.js';
import {
  MessageEffectsCapabilitiesSchema,
  NativeMediaEditCapabilitiesSchema,
  NativeMediaReplacementSchema,
  NativeMediaUpdateSchema,
  OutboundMessageSchema,
} from '../src/cmaeg/types.js';

it('round-trips media update contracts and preserves omission versus clear', () => {
  const examples = JSON.parse(
    readFileSync(
      new URL('../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json', import.meta.url),
      'utf8',
    ),
  ) as Record<string, readonly unknown[]>;
  for (const [name, parse] of Object.entries({
    NativeMediaEditCapabilities: NativeMediaEditCapabilitiesSchema,
    NativeMediaReplacement: NativeMediaReplacementSchema,
    NativeMediaUpdate: NativeMediaUpdateSchema,
  }))
    for (const example of examples[name] ?? []) expect(parse(example)).toEqual(example);
  const message = { native_media_update: { caption: '' } };
  expect(OutboundMessageSchema(message)).toEqual(message);
  for (const field of ['replacement', 'caption', 'text']) {
    expect(() => NativeMediaUpdateSchema({ [field]: null })).toThrow(field);
  }
  expect(() => OutboundMessageSchema({ native_media_update: null })).toThrow('native_media_update');
  const legacy = {
    delete: true,
    reaction: true,
    scope: 'confirmed_native_bot_message',
    max_reactions: 1,
  };
  expect(MessageEffectsCapabilitiesSchema(legacy)).toEqual(legacy);
});

it('sends the exact update key once and retains the returned next reference', async () => {
  const response = {
    delivery: { outcome: 'confirmed' },
    rendering: { outcome: 'rendered' },
    message_ref: 'next/revision',
  };
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValue(
      new Response(JSON.stringify(response), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    );
  const client = new Cmaeg({
    baseUrl: 'https://gateway.example',
    credential: () => 'fixture-custody',
    fetch,
  });
  const message = { native_media_update: { caption: '', text: 'visible content' } };
  expect(
    await client.updateMessage('opaque/ref', message, { idempotencyKey: 'media-edit-1' }),
  ).toEqual(response);
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(fetch.mock.calls[0]?.[0]).toBe('https://gateway.example/v1/messages/opaque%2Fref/update');
  expect(new Headers(fetch.mock.calls[0]?.[1]?.headers).get('Idempotency-Key')).toBe(
    'media-edit-1',
  );
  expect(JSON.parse(fetch.mock.calls[0]?.[1]?.body as string)).toEqual(message);
});
