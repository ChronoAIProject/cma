import test from 'node:test';
import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {setImmediate as nextTurn} from 'node:timers/promises';
import {e1Journey, releaseBody, releaseWinner, assertReleasedState, pendingReply} from './e1.mjs';
import {cases} from './index.mjs';
import {executeCase} from '../../runner.mjs';

const ctx = {assert: (condition, code) => assert.ok(condition, code)};
const preview = {partition_id: randomUUID(), head: randomUUID(), epoch: 2, version: 4,
  phase: 'unknown', event_expires_at: 1, releasable: true};
const terminal = {head: preview.head, terminal: true, inflight: 0, evidence_sha256: 'a'.repeat(64)};

function releaseFixture() {
  const commands = [randomUUID(), randomUUID()].map(id => releaseBody(ctx, preview, terminal, id));
  const result = {operation_id: commands[1].operation_id, released_head: preview.head,
    disposition: 'unknown_abandoned', epoch: 3, released_at_ms: 1000};
  return {commands, result};
}

test('E1 supplement dispatches its steps and remains outside all79 frozen registrations', async () => {
  assert.equal(cases.length, 79); assert.ok(cases.every(c => c.id !== e1Journey.id));
  const steps = []; await e1Journey.run({step: async id => {steps.push(id);}});
  assert.deepEqual(steps, e1Journey.steps);
  const result = await executeCase(e1Journey, {id: 'EG-E1', case_id: e1Journey.id, family: 'eg',
    required_steps: e1Journey.steps}, {phase: 'steady', iteration: 1, am: 'a',
    source: 'a'.repeat(40), image: 'local@sha256:' + 'b'.repeat(64), fixtures: {}});
  assert.equal(result.status, 'BLOCKED'); assert.equal(result.requests.length, 0);
});

test('E1 release refuses expired-only, foreign or inflight terminal claims and stale head shapes', () => {
  for (const value of [{...terminal, terminal: false}, {...terminal, inflight: 1},
    {...terminal, head: randomUUID()}, {...terminal, evidence_sha256: undefined}])
    assert.throws(() => releaseBody(ctx, preview, value, randomUUID()), /provider_terminal_proof_required/);
  for (const value of [{...preview, releasable: false}, {...preview, phase: 'accepted'},
    {...preview, version: 0}, {...preview, head: 'not-a-durable-identity'}])
    assert.throws(() => releaseBody(ctx, value, terminal, randomUUID()), /exact_releasable_head/);
});

test('E1 two-replica race requires one actual winner and exact disposition identity', () => {
  const {commands, result} = releaseFixture();
  const replies = [{status: 409, body: {}}, {status: 200, body: result}];
  assert.equal(releaseWinner(ctx, replies, commands).winner, 1);
  for (const changed of [[{status: 200, body: result}, replies[1]],
    [replies[0], {...replies[1], body: {...result, released_head: randomUUID()}}],
    [replies[0], {...replies[1], body: {...result, disposition: 'delivered'}}],
    [replies[0], {...replies[1], body: {...result, epoch: preview.epoch}}]])
    assert.throws(() => releaseWinner(ctx, changed, commands), /e1_/);
});

test('E1 disposition must pop one head and preserve request, tombstone and authenticated audit', () => {
  const {commands, result} = releaseFixture(), command = commands[1];
  const before = {queue: [preview.head, randomUUID(), randomUUID()], request_hash: 'b'.repeat(64), body_sha256: 'c'.repeat(64)};
  const after = {queue: before.queue.slice(1)};
  const journal = {phase: {state: 'unknown_abandoned', operation_id: command.operation_id}, tombstone: true,
    request_hash: before.request_hash, body_sha256: before.body_sha256};
  const actor = 'd'.repeat(64), audit = {kind: 'cmaeg_unknown_abandoned_v1', request: command, result,
    original_phase: {state: 'unknown'}, request_hash: before.request_hash, actor_subject_sha256: actor};
  assertReleasedState(ctx, before, after, journal, audit, command, result, actor);
  assert.throws(() => assertReleasedState(ctx, before, {queue: before.queue.slice(2)}, journal, audit,
    command, result, actor), /advances_one_head/);
  assert.throws(() => assertReleasedState(ctx, before, after, {...journal, tombstone: false}, audit,
    command, result, actor), /preserves_request_and_tombstone/);
  assert.throws(() => assertReleasedState(ctx, before, after, journal, {...audit, actor_subject_sha256: 'e'.repeat(64)},
    command, result, actor), /authenticated_audit/);
});

test('E1 delayed reply does not poll definite conflicts or missing Retry-After', async () => {
  for (const [detail, retryAfterSeconds] of [['reply_idempotency_conflict', 2], ['routed_reply_in_progress', null]]) {
    let attempts = 0;
    const lease = {replicas: [{slot: 'a', instance: 'eg-a'}], async request() {attempts++; return {
      status: 409, body: {detail, retry_after_seconds: 2}, retryAfterSeconds, instance: 'eg-a', requestId: 'request',
    };}};
    await assert.rejects(pendingReply({...ctx, am: 'a', observe: async (_label, op) => op()}, lease,
      {method: 'POST', path: '/v1/reply-targets/synthetic/messages', key: randomUUID(), body: {text: 'fixture'}}),
    /only_pending_reply_is_pollable/);
    assert.equal(attempts, 1);
  }
});

test('E1 cancellation prevents another pending poll from reaching transport', async () => {
  let requests = 0;
  const controller = new AbortController(); controller.abort();
  await assert.rejects(pendingReply({...ctx, signal: controller.signal}, {request: async () => {requests++;}}, {}),
    /e1_reply_poll_budget/);
  assert.equal(requests, 0);
});

for (const interruption of ['cancellation', 'budget exhaustion']) test(
  `E1 joins and retains a delayed response after ${interruption} without accepting it`, async () => {
    const controller = new AbortController(), observations = [];
    let remaining = 30000, requests = 0, settled = false, release, entered;
    const gate = new Promise(resolve => {release = resolve;});
    const started = new Promise(resolve => {entered = resolve;});
    const response = {status: 200, body: {delivery: {outcome: 'confirmed'}},
      instance: 'eg-a', requestId: 'synthetic-late-reply', eg_receipt: {synthetic: true}};
    const lease = {replicas: [{slot: 'a', instance: 'eg-a'}], async request() {
      requests++; entered(); await gate; return response;
    }};
    const context = {...ctx, am: 'a', signal: controller.signal, remainingMs: () => remaining,
      observe: async (label, operation) => {
        const value = await operation(); observations.push({label, value}); return value;
      }};
    const result = pendingReply(context, lease, {method: 'POST', path: '/synthetic-reply'})
      .then(value => {settled = true; return {value};}, error => {settled = true; return {error};});
    await started;
    if (interruption === 'cancellation') controller.abort();
    else remaining = 0;
    await nextTurn();
    const settledBeforeResponse = settled;
    release();
    const outcome = await result;
    assert.equal(settledBeforeResponse, false, 'join the original request before cleanup');
    assert.match(outcome.error?.message ?? '', /e1_reply_poll_budget/);
    assert.equal(requests, 1, 'never retry the interrupted request');
    assert.equal(observations.find(entry => entry.label === 'eg_serving_receipt')?.value.receipt,
      response.eg_receipt, 'retain the late acknowledgement for reconciliation');
  });

test('E1 accepts a timely confirmed delayed response', async () => {
  const body = {delivery: {outcome: 'confirmed'}}, controller = new AbortController();
  const lease = {replicas: [{slot: 'a', instance: 'eg-a'}], async request() {
    return {status: 200, body, instance: 'eg-a', requestId: 'synthetic-timely-reply'};
  }};
  const result = await pendingReply({...ctx, am: 'a', signal: controller.signal, remainingMs: () => 30000,
    observe: async (_label, operation) => operation()}, lease, {method: 'POST', path: '/synthetic-reply'});
  assert.equal(result, body);
});
