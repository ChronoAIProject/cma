import { expect, it } from 'vitest';
import { ProtocolError } from '../src/index.js';
import {
  agent,
  response,
  intent,
  observation,
  receipt,
  harness,
  json,
  set,
} from './queued-cancellation-v2-fixtures.js';

it.each([
  ['object', 'foreign'],
  ['command_revision', 0],
  ['command_revision', Number.MAX_SAFE_INTEGER + 1],
  ['cancellable', 'yes'],
  ['response.receipt.agent_id', 'foreign'],
  ['response.receipt.response_id', null],
  ['response.receipt.response_id', 'foreign'],
  ['response.status', 'in_progress'],
  ['response.turn_sequence', 5],
  ['response.failure', 'queue_timeout'],
  ['response.actor', 'forged'],
])('rejects inconsistent queued cancellation observation %s', async (path, value) => {
  const body = observation();
  set(body, path, value);
  const t = harness();
  t.fetch.mockResolvedValueOnce(json(body));
  await expect(t.api.observeQueuedCancellation(agent, response)).rejects.toBeInstanceOf(
    ProtocolError,
  );
});

it.each([
  ['object', 'foreign'],
  ['target.response_id', 'foreign'],
  ['target.source_command_id', 'foreign'],
  ['target.expected_command_revision', 2],
  ['target.actor', 'forged'],
  ['cancellation.receipt.agent_id', 'foreign'],
  ['cancellation.receipt.command_id', intent.source_command_id],
  ['cancellation.receipt.response_id', response],
  ['cancellation.status', 'queued'],
  ['cancellation.turn_sequence', 4],
  ['cancellation.delivery', 'write_confirmed'],
  ['cancellation.observed_cursor', 5],
  ['cancellation.failure', 'queue_timeout'],
  ['response.receipt.agent_id', 'foreign'],
  ['response.receipt.response_id', 'foreign'],
  ['response.receipt.command_id', 'foreign'],
])(
  'rejects cancellation receipt that changes source or completion facts: %s',
  async (path, value) => {
    const body = receipt();
    set(body, path, value);
    const t = harness();
    t.fetch.mockResolvedValueOnce(json(body));
    await expect(
      t.api.cancelQueuedResponse(agent, response, intent, { idempotencyKey: 'saved' }),
    ).rejects.toBeInstanceOf(ProtocolError);
  },
);

it('requires explicit nulls on cancellation receipts and accepts inactive bound observations', async () => {
  const t = harness();
  const broken = receipt();
  delete (broken.cancellation as Record<string, unknown>).delivery;
  t.fetch.mockResolvedValueOnce(json(broken));
  await expect(
    t.api.cancelQueuedResponse(agent, response, intent, { idempotencyKey: 'saved' }),
  ).rejects.toBeInstanceOf(ProtocolError);
  const bound = observation(false);
  set(bound, 'response.status', 'in_progress');
  set(bound, 'response.turn_sequence', 4);
  set(bound, 'response.delivery', 'dispatch_fenced');
  t.fetch.mockResolvedValueOnce(json(bound));
  expect((await t.api.observeQueuedCancellation(agent, response)).cancellable).toBe(false);
});

it('accepts older discovery and strictly validates the optional queued cancellation extension', async () => {
  const { loadCorpus } = await import('./public-contract/context.js');
  const original = structuredClone(
    loadCorpus().find(
      (entry) => entry.operation === 'GET /api/v2' && entry.response.status === 200,
    )!.response.body,
  ) as Record<string, unknown>;
  delete original.queued_cancellation;
  const t = harness();
  t.fetch.mockResolvedValueOnce(json(original));
  expect((await t.api.readCapabilities()).queued_cancellation).toBeUndefined();
  const capability = {
    path: '/api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation',
    observe: 'GET',
    submit: 'POST',
    idempotency_header: 'Idempotency-Key',
    idempotency_required: true,
    target: 'unbound_response_command',
    completion: 'source_cancelled',
  };
  t.fetch.mockResolvedValueOnce(json({ ...original, queued_cancellation: capability }));
  expect((await t.api.readCapabilities()).queued_cancellation).toEqual(capability);
  for (const delta of [
    { idempotency_required: false },
    { target: 'bound_response_turn' },
    { completion: 'command_delivery' },
    { private: true },
  ]) {
    t.fetch.mockResolvedValueOnce(
      json({ ...original, queued_cancellation: { ...capability, ...delta } }),
    );
    await expect(t.api.readCapabilities()).rejects.toBeInstanceOf(ProtocolError);
  }
});
