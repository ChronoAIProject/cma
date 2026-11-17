import test from 'node:test';
import assert from 'node:assert/strict';
import {campaign, percentile, sameHome, summarize} from './engine.mjs';
import {PauseAdapter} from './http.mjs';

const home = {sandbox_id: 's', native_uid: 'n', home_identity: 'h',
  pvcs: [{name: 'home', uid: 'pvc-1', volume: 'volume-1'}]};
function fixture(overrides = {}) {
  const calls = [];
  const methods = {prepare: async () => ({home}), alwaysOnException: async () => {},
    awaitAutomaticPause: async () => ({runtime_pods: 0}), wakeWithMessage: async () => ({}),
    home: async () => structuredClone(home), holdActiveTurn: async () => ({interrupted: false}),
    completeTurn: async () => ({receipt: 'receipt'}), verifyRetention: async () => {},
    verifyAllReceipts: async () => {}, ...overrides};
  return {calls, adapter: Object.fromEntries(Object.entries(methods).map(([name, fn]) =>
    [name, async (...args) => {calls.push(name); return fn(...args);}]))};
}
test('nearest-rank p95 includes slow samples without interpolation', () => {
  assert.equal(percentile(Array.from({length: 100}, (_, i) => i + 1), 0.95), 95);
  assert.throws(() => percentile([], 0.95));
  assert.throws(() => percentile([NaN], 0.95));
  assert.throws(() => percentile([1], 0));
});
test('home verification refuses lost claims, identity drift, or replacement PVC', () => {
  sameHome(home, structuredClone(home));
  for (const changed of [{...home, home_identity: null}, {...home, native_uid: 'replacement'},
    {...home, pvcs: [{name: 'home', uid: 'different', volume: 'volume-1'}]}]) {
    assert.throws(() => sameHome(home, changed));
  }
  assert.throws(() => sameHome({...home, pvcs: []}, {...home, pvcs: []}));
});
test('qualification requires every requested sample and a passing p95', () => {
  assert.equal(summarize([], 100).verdict, 'FAIL');
  assert.equal(summarize([{verdict: 'PASS', resume_ms: 1}], 100).verdict, 'FAIL');
  assert.equal(summarize([{verdict: 'PASS', resume_ms: 30001}], 1).verdict, 'FAIL');
  assert.equal(summarize([{verdict: 'FAIL'}], 1).failed, 1);
  assert.equal(summarize([{verdict: 'PASS', resume_ms: 30000}], 1).verdict, 'PASS');
});
test('100 cycles keep strict ordering, test active turn, and check every receipt', async () => {
  const {adapter, calls} = fixture(); const progress = []; let clock = 0;
  const result = await campaign(adapter, {record: async p => progress.push(p.summary), now: () => ++clock});
  assert.equal(result.summary.passed, 100);
  assert.equal(progress.length, 100);
  assert.equal(calls.filter(x => x === 'holdActiveTurn').length, 1);
  assert.deepEqual(calls.slice(0, 8), ['prepare', 'alwaysOnException', 'awaitAutomaticPause',
    'wakeWithMessage', 'home', 'holdActiveTurn', 'completeTurn', 'verifyRetention']);
  assert.equal(calls.at(-1), 'verifyAllReceipts');
});
test('failed real cycle is preserved and never retried as a replacement sample', async () => {
  const {adapter, calls} = fixture({verifyRetention: async () => {throw new Error('receipt lost');}});
  let saved;
  await assert.rejects(campaign(adapter, {count: 3, record: async p => {saved = p;}}), /receipt lost/);
  assert.equal(saved.cycles.length, 1); assert.equal(saved.summary.failed, 1);
  assert.equal(calls.filter(x => x === 'wakeWithMessage').length, 1);
});
test('identity failure cannot be masked by successful reply', async () => {
  const {adapter, calls} = fixture({home: async () => ({...home, native_uid: 'replacement'})});
  await assert.rejects(campaign(adapter, {count: 1, record: async () => {}}), /home changed/);
  assert(!calls.includes('completeTurn'));
  await assert.rejects(campaign(adapter, {count: 0}));
});
test('HTTP adapter refuses remote API and control origins before any request', () => {
  assert.throws(() => new PauseAdapter({ams: {a: 'https://production.invalid'}}, () => {}), /local API/);
  assert.throws(() => new PauseAdapter({ams: {a: 'http://127.0.0.1:1234'},
    control_origin: 'http://remote.invalid'}, () => {}), /local control/);
});
