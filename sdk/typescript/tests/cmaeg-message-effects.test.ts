import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { Cmaeg } from '../src/cmaeg/index.js';
import {
  EmptyEffectRequestSchema,
  MessageEffectReceiptSchema,
  MessageEffectsCapabilitiesSchema,
  MessageReactionRequestSchema,
  TelegramReactionSchema,
} from '../src/cmaeg/types.js';

describe('native Telegram message effects', () => {
  it('round-trips every effect contract example and exposes explicit unsupported capabilities', async () => {
    const examples = JSON.parse(
      readFileSync(
        new URL('../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json', import.meta.url),
        'utf8',
      ),
    ) as Record<string, readonly unknown[]>;
    for (const [name, parse] of Object.entries({
      EmptyEffectRequest: EmptyEffectRequestSchema,
      MessageEffectReceipt: MessageEffectReceiptSchema,
      MessageEffectsCapabilities: MessageEffectsCapabilitiesSchema,
      MessageReactionRequest: MessageReactionRequestSchema,
      TelegramReaction: TelegramReactionSchema,
    })) {
      expect(examples[name]?.length).toBeGreaterThan(0);
      for (const example of examples[name] ?? []) expect(parse(example)).toEqual(example);
    }
    const unsupported = {
      delete: false,
      reaction: false,
      scope: 'confirmed_native_bot_message',
      max_reactions: 0,
    };
    const fetch = vi.fn<typeof globalThis.fetch>().mockResolvedValue(
      new Response(JSON.stringify(unsupported), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    );
    const client = new Cmaeg({
      baseUrl: 'https://gateway.example',
      credential: () => 'fixture-custody',
      fetch,
    });
    expect(await client.readMessageCapabilities('opaque/ref')).toEqual(unsupported);
    expect(fetch.mock.calls[0]?.[0]).toBe(
      'https://gateway.example/v1/messages/opaque%2Fref/capabilities',
    );
    expect(fetch.mock.calls[0]?.[1]?.method).toBe('GET');
  });

  it('forwards only sealed references, explicit reaction input and stable idempotency keys', async () => {
    const delivery = { delivery: { outcome: 'unknown', reason: 'telegram_response_unconfirmed' } };
    const fetch = vi.fn<typeof globalThis.fetch>().mockResolvedValue(
      new Response(JSON.stringify(delivery), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    );
    const client = new Cmaeg({
      baseUrl: 'https://gateway.example',
      credential: () => 'fixture-custody',
      fetch,
    });
    await client.deleteMessage('opaque/ref', 'delete-key');
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(fetch.mock.calls[0]?.[0]).toBe(
      'https://gateway.example/v1/messages/opaque%2Fref/delete',
    );
    expect(new Headers(fetch.mock.calls[0]?.[1]?.headers).get('Idempotency-Key')).toBe(
      'delete-key',
    );
    expect(fetch.mock.calls[0]?.[1]?.body).toBe('{}');
    fetch.mockResolvedValueOnce(
      new Response(JSON.stringify(delivery), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    );
    await client.setMessageReaction('opaque/ref', 'reaction-key', { reaction: [] });
    expect(fetch).toHaveBeenCalledTimes(2);
    expect(fetch.mock.calls[1]?.[0]).toBe(
      'https://gateway.example/v1/messages/opaque%2Fref/reaction',
    );
    expect(new Headers(fetch.mock.calls[1]?.[1]?.headers).get('Idempotency-Key')).toBe(
      'reaction-key',
    );
    expect(fetch.mock.calls[1]?.[1]?.body).toBe('{"reaction":[]}');
  });
});
