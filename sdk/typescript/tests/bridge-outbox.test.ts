import { expect, it } from 'vitest';
import { ProtocolError, ValidationError } from '../src/index.js';
import { loadCorpus } from './public-contract/context.js';
import { transport } from './resource-fixtures.js';
import * as schema from '../src/bridges/outbox-schema.js';
const corpus = loadCorpus();
const fixture = (name: string) => corpus.find((item) => item.id === `bridge-outbox-${name}`)!;
const delivery = schema.page(fixture('list-unknown').response.body).items[0]!;
const queued = schema.recoveryReceipt(fixture('retry-explicit').response.body);
const request = {
  item_id: delivery.id,
  expected_revision: delivery.revision,
  expected_bridge_revision: 2,
  action: { kind: 'retry' as const, accept_duplicate_risk: true },
};

it('refuses delivery source substitutions, secret fields and inconsistent history', async () => {
  const test = transport();
  for (const wrong of [
    { ...delivery, bridge_id: 'another' },
    { ...delivery, id: `bro_${'b'.repeat(64)}` },
    { ...delivery, text: 'secret' },
    { ...delivery, credential: {} },
    { ...delivery, source: { chat_id: 'elsewhere' } },
    { ...delivery, chunk_index: delivery.chunk_count },
    { ...delivery, attempts: [{ ...delivery.attempts[0], number: 2 }] },
    { ...delivery, next_retry_at_ms: 99 },
    { ...delivery, updated_at_ms: 0 },
  ]) {
    test.answer(wrong);
    await expect(
      test.client.bridges.readDelivery(delivery.bridge_id, delivery.id),
    ).rejects.toBeInstanceOf(ProtocolError);
  }
  test.answer(delivery);
  expect(await test.client.bridges.readDelivery(delivery.bridge_id, delivery.id)).toEqual(delivery);
});
it('binds recovery receipts to original item, action, revision and honest queued state', async () => {
  const test = transport();
  const options = { idempotencyKey: 'explicit-retry' };
  for (const wrong of [
    { ...queued, bridge_id: 'elsewhere' },
    { ...queued, action: { kind: 'abandon' } },
    { ...queued, result: { ...queued.result, status: 'sent' } },
    { ...queued, result: { ...queued.result, revision: queued.result.revision + 1 } },
    { ...queued, actor: { kind: 'channel', bridge_id: delivery.bridge_id } },
    { ...queued, action: { kind: 'retry', accept_duplicate_risk: false } },
  ]) {
    test.answer(wrong);
    await expect(
      test.client.bridges.recoverDelivery(delivery.bridge_id, request, options),
    ).rejects.toBeInstanceOf(ProtocolError);
  }
  test.answer(queued);
  expect(await test.client.bridges.recoverDelivery(delivery.bridge_id, request, options)).toEqual(
    queued,
  );
  expect(queued.result.status).toBe('pending');
  expect(queued.result.attempts[0]?.status).toBe('unknown');
});
it('rejects nested recovery routing fields and nonadvancing pages before exposing data', async () => {
  const test = transport();
  expect(() =>
    test.client.bridges.recoverDelivery(
      delivery.bridge_id,
      { ...request, action: { kind: 'abandon', destination: 'elsewhere' } } as never,
      { idempotencyKey: 'changed' },
    ),
  ).toThrow(ValidationError);
  test.answer({ items: [delivery], next_cursor: delivery.id });
  await expect(
    test.client.bridges.listDeliveries(delivery.bridge_id, { cursor: delivery.id }),
  ).rejects.toBeInstanceOf(ProtocolError);
});
