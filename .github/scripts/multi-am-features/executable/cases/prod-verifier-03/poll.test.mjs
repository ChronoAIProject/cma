import test from 'node:test';
import assert from 'node:assert/strict';
import {setImmediate as nextTurn} from 'node:timers/promises';
import {poll} from './helpers.mjs';

const context = {assert: (condition, code) => assert.ok(condition, code)};
const code = 'synthetic_poll_timeout';

test('cancelled EG polling never starts a fixture read', async () => {
  const controller = new AbortController(); controller.abort();
  let reads = 0;
  await assert.rejects(poll({...context, signal: controller.signal}, async () => {
    reads++; return {ready: true};
  }, value => value.ready, code), new RegExp(code));
  assert.equal(reads, 0);
});

for (const [name, remainingMs, milliseconds] of [
  ['case budget', () => 0, 90000], ['poll deadline', () => 90000, 0],
]) test(`expired ${name} prevents the first EG poll read`, async () => {
  let reads = 0;
  await assert.rejects(poll({...context, remainingMs}, async () => {
    reads++; return {ready: true};
  }, value => value.ready, code, milliseconds), new RegExp(code));
  assert.equal(reads, 0);
});

test('EG poll cancellation joins the in-flight read before returning to cleanup', async () => {
  const controller = new AbortController();
  let unblock, notifyStarted, reads = 0, predicates = 0, cleanups = 0;
  const gate = new Promise(resolve => {unblock = resolve;});
  const started = new Promise(resolve => {notifyStarted = resolve;});
  const execution = (async () => {
    try {
      return await poll({...context, signal: controller.signal}, async () => {
        reads++; notifyStarted(); await gate; return {ready: true};
      }, value => {predicates++; return value.ready;}, code);
    } finally {cleanups++;}
  })();
  // Attach the failure observer before releasing the gated read.
  const outcome = execution.then(value => ({value}), error => ({error}));
  await started;
  controller.abort();
  await nextTurn();
  const earlyCleanup = cleanups;
  unblock();
  const result = await outcome;
  assert.equal(earlyCleanup, 0, 'cancellation must not abandon a live fixture operation');
  assert.match(result.error?.message ?? '', new RegExp(code));
  assert.equal(reads, 1); assert.equal(predicates, 0); assert.equal(cleanups, 1);
});

test('EG poll refuses a ready result that arrives after the case budget', async () => {
  let remaining = 1000, reads = 0, predicates = 0;
  await assert.rejects(poll({...context, remainingMs: () => remaining}, async () => {
    reads++; remaining = 0; return {ready: true};
  }, value => {predicates++; return value.ready;}, code), new RegExp(code));
  assert.equal(reads, 1); assert.equal(predicates, 0);
});

test('EG poll cancellation during its interval prevents another read', async () => {
  const controller = new AbortController();
  let reads = 0;
  const execution = poll({...context, signal: controller.signal}, async () => {
    reads++; return {ready: reads > 1};
  }, value => value.ready, code);
  const outcome = execution.then(value => ({value}), error => ({error}));
  // The first read and its predicate finish before cancellation of the interval.
  await nextTurn();
  controller.abort();
  const result = await outcome;
  assert.match(result.error?.message ?? '', new RegExp(code));
  assert.equal(reads, 1);
});

test('EG poll deadline during its interval prevents another read', async () => {
  let reads = 0;
  await assert.rejects(poll(context, async () => {
    reads++; return {ready: reads > 1};
  }, value => value.ready, code, 20), new RegExp(code));
  assert.equal(reads, 1);
});

test('EG poll preserves a timely result and propagates an original read failure', async () => {
  const ready = {ready: true, receipt: 'synthetic'};
  assert.equal(await poll(context, async () => ready, value => value.ready, code), ready);
  const failure = Error('synthetic_fixture_read_failed');
  await assert.rejects(poll(context, async () => {throw failure;}, () => false, code),
    error => error === failure);
});
