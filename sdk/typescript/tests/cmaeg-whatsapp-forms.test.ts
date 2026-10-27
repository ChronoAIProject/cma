import { expect, it, vi } from 'vitest';
import { Cmaeg } from '../src/cmaeg/index.js';
import { FormPreparationReceiptSchema } from '../src/cmaeg/types.js';

it('distinguishes asset uncertainty from a customer attempt', () => {
  const common = { preparation_id: 'opaque', expires_at: 1900000000 };
  const receipt = {
    delivery: { outcome: 'unknown', reason: 'unconfirmed' },
    rendering: { outcome: 'rendered' },
  };
  for (const state of [
    { status: 'pending', retry_after_ms: 1000 },
    { status: 'ready', preparation_ref: 'ready' },
    ...['declined', 'uncertain', 'revoked'].map((status) => ({ status, reason: 'source' })),
  ]) {
    const value = { ...common, customer_message: 'not_sent', ...state };
    expect(FormPreparationReceiptSchema(value)).toEqual(value);
  }
  const sent = { ...common, customer_message: 'attempted', status: 'delivery', receipt };
  expect(FormPreparationReceiptSchema(sent)).toEqual(sent);
  expect(() => FormPreparationReceiptSchema({ ...sent, customer_message: 'not_sent' })).toThrow();
});
it('prepares, reads, and explicitly commits the exact message with distinct stable keys', async () => {
  const common = { preparation_id: 'opaque', expires_at: 1900000000, customer_message: 'not_sent' };
  const answers = [
    { ...common, status: 'pending', retry_after_ms: 1000 },
    { ...common, status: 'ready', preparation_ref: 'ready' },
    { delivery: { outcome: 'confirmed' }, rendering: { outcome: 'rendered' } },
  ];
  const fetch = vi.fn<typeof globalThis.fetch>().mockImplementation(
    async () =>
      new Response(JSON.stringify(answers.shift()), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
  );
  const client = new Cmaeg({
    baseUrl: 'https://gateway.example',
    credential: () => 'fixture-custody',
    fetch,
  });
  const message = { text: 'Form prompt' };
  await client.prepareFormReply('event/ref', message, 'prepare-key');
  await client.readFormPreparation('event/ref');
  await client.replyToEvent('event/ref', message, {
    idempotencyKey: 'commit-key',
    formPreparationRef: 'ready',
  });
  expect(fetch).toHaveBeenCalledTimes(3);
  expect(fetch.mock.calls[0]![0]).toBe(
    'https://gateway.example/v1/events/event%2Fref/form-preparation',
  );
  expect(new Headers(fetch.mock.calls[0]![1]?.headers).get('Idempotency-Key')).toBe('prepare-key');
  expect(fetch.mock.calls[1]![1]?.method).toBe('GET');
  expect(new Headers(fetch.mock.calls[2]![1]?.headers).get('Idempotency-Key')).toBe('commit-key');
  expect(new Headers(fetch.mock.calls[2]![1]?.headers).get('Form-Preparation-Ref')).toBe('ready');
  expect(JSON.parse(fetch.mock.calls[2]![1]?.body as string)).toEqual(message);
});
