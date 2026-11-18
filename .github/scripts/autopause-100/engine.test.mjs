import test from 'node:test';
import assert from 'node:assert/strict';
import {campaign, percentile, sameHome, summarize, validateLocalOrigins} from './engine.mjs';

const pause = {operating_mode: 'Suspended', generation: 2, pods: [],
  conditions: [{type: 'Suspended', status: 'True', observedGeneration: 2}]};
const home = {sandbox_id: 's', native_uid: 'n', home_identity: 'h',
  pvcs: [{name: 'home', uid: 'pvc-1', volume: 'volume-1'}]};
function fixture(overrides = {}) {
  const calls = [];
  const methods = {validate: async () => {}, prepare: async () => ({home}), alwaysOnException: async () => {},
    awaitAutomaticPause: async () => structuredClone(pause), wakeWithMessage: async () => ({}),
    home: async () => structuredClone(home), holdActiveTurn: async () => ({interrupted: false, samples: 18, hold_ms: 90000}),
    completeTurn: async () => ({receipt: 'receipt'}), verifyRetention: async () => {},
    verifyAllReceipts: async () => {}, ...overrides};
  return {calls, adapter: {origins: {ams: {a: 'http://127.0.0.1:4100'}, control_origin: 'http://127.0.0.1:4200'},
    ...Object.fromEntries(Object.entries(methods).map(([name, fn]) =>
      [name, async (...args) => {calls.push(name); return fn(...args); }]))}};
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
test('sample success requires every requested cycle and never qualifies the release', () => {
  for (const requested of [0, -1, 101, NaN]) assert.throws(() => summarize([], requested));
  assert.equal(summarize([], 100).verdict, 'FAIL');
  assert.equal(summarize([{verdict: 'PASS', resume_ms: 1}], 100).verdict, 'FAIL');
  assert.equal(summarize([{verdict: 'PASS', resume_ms: 30001}], 1).verdict, 'FAIL');
  assert.equal(summarize([{verdict: 'FAIL'}], 1).failed, 1);
  assert.equal(summarize([{verdict: 'PASS', resume_ms: 30000}], 1).verdict, 'PASS');
  assert.equal(summarize([{verdict: 'PASS', resume_ms: 1}], 1).qualification, 'NOT_ASSESSED');
});
test('100 cycles keep strict ordering, test active turn, and check every receipt', async () => {
  const {adapter, calls} = fixture(); const progress = []; let clock = 0;
  const result = await campaign(adapter, {record: async p => progress.push(p.summary), now: () => ++clock});
  assert.equal(result.summary.passed, 100);
  assert.equal(progress.length, 101);
  assert.equal(result.final_verification, 'PASS');
  assert.equal(result.summary.qualification, 'NOT_ASSESSED');
  assert.equal(calls.filter(x => x === 'holdActiveTurn').length, 1);
  assert.deepEqual(calls.slice(0, 9), ['validate', 'prepare', 'alwaysOnException', 'awaitAutomaticPause',
    'wakeWithMessage', 'home', 'holdActiveTurn', 'completeTurn', 'verifyRetention']);
  assert.equal(calls.at(-1), 'verifyAllReceipts');
});
test('failed real cycle is preserved and never retried as a replacement sample', async () => {
  const {adapter, calls} = fixture({verifyRetention: async () => {throw new Error('receipt lost');}});
  let saved;
  await assert.rejects(campaign(adapter, {count: 3, record: async p => {saved = p;}}), /retention_failed/);
  assert.equal(saved.cycles.length, 1); assert.equal(saved.summary.failed, 1);
  assert.equal(calls.filter(x => x === 'wakeWithMessage').length, 1);
});
test('identity failure cannot be masked by successful reply', async () => {
  const {adapter, calls} = fixture({home: async () => ({...home, native_uid: 'replacement'})});
  await assert.rejects(campaign(adapter, {count: 1, record: async () => {}}), /home_failed/);
  assert(!calls.includes('completeTurn'));
  await assert.rejects(campaign(adapter, {count: 0}));
});
test('adapter validation refuses unauthorized fixtures before setup', async () => {
  for (const refusal of ['candidate_not_verified', 'fixture_not_authorized']) {
    const {adapter, calls} = fixture({validate: async () => {throw Error(refusal);}});
    let saved;
    await assert.rejects(campaign(adapter, {count: 1, record: async packet => {saved = packet;}}),
      /preparation_failed/);
    assert.deepEqual(calls, ['validate']);
    assert.equal(saved.verdict, 'FAIL');
    assert.equal(saved.summary.attempted, 0);
  }
});

test('remote or malformed API/control origins are refused before any adapter call', async () => {
  for (const origin of ['https://production.invalid', 'http://remote.invalid', 'bad',
    'http://user@localhost', 'http://localhost/path', 'http://localhost/?query', 'http://localhost/#fragment']) {
    for (const key of ['api', 'control']) {
      const {adapter, calls} = fixture();
      if (key === 'api') adapter.origins.ams.a = origin;
      else adapter.origins.control_origin = origin;
      await assert.rejects(campaign(adapter, {count: 1, record: async () => {}}), /local_api|local_control/);
      assert.deepEqual(calls, []);
    }
  }
  for (const origins of [undefined, {}, {ams: {}}]) assert.throws(() => validateLocalOrigins(origins));
  for (const host of ['127.0.0.1', 'localhost', '[::1]']) {
    validateLocalOrigins({ams: {a: `http://${host}:4100`}, control_origin: `http://${host}:4200`});
  }
});

test('missing adapter methods and evidence recorder refuse before any setup', async () => {
  const {adapter, calls} = fixture();
  await assert.rejects(campaign(adapter), /private_recorder_required/);
  for (const key of Object.keys(adapter).filter(key => key !== 'origins')) {
    await assert.rejects(campaign({...adapter, [key]: undefined}, {record: async () => {}}),
      /adapter_contract_incomplete/);
  }
  assert.deepEqual(calls, []);
});

test('suspension, active-turn protection and timing need actual positive evidence', async () => {
  for (const overrides of [
    {awaitAutomaticPause: async () => ({...pause, pods: [{}]})},
    {awaitAutomaticPause: async () => ({...pause, generation: 3})},
    {holdActiveTurn: async () => ({interrupted: true})},
    {holdActiveTurn: async () => ({})},
    {holdActiveTurn: async () => ({interrupted: false, samples: 0, hold_ms: 100})},
    {holdActiveTurn: async () => ({interrupted: false, samples: 2, hold_ms: 0})},
  ]) {
    const {adapter, calls} = fixture(overrides); let saved;
    await assert.rejects(campaign(adapter, {count: 1, record: async packet => {saved = packet;}}));
    assert.equal(saved.cycles.length, 1); assert.equal(saved.cycles[0].verdict, 'FAIL');
    assert(!calls.includes('verifyAllReceipts'));
  }
  const {adapter} = fixture();
  await assert.rejects(campaign(adapter, {count: 1, now: () => NaN, record: async () => {}}), /message_wake_failed/);
});

test('every failing stage retains a failed attempt without raw provider errors', async () => {
  for (const method of ['awaitAutomaticPause', 'wakeWithMessage', 'home', 'holdActiveTurn',
    'completeTurn', 'verifyRetention']) {
    const {adapter, calls} = fixture({[method]: async () => {throw Error('private-provider-payload');}});
    let saved;
    await assert.rejects(campaign(adapter, {count: 3, record: async packet => {saved = packet;}}));
    assert.equal(saved.verdict, 'FAIL'); assert.equal(saved.summary.attempted, 1);
    assert.equal(saved.summary.failed, 1);
    assert(!JSON.stringify(saved).includes('private-provider-payload'));
    assert(calls.filter(name => name === method).length === 1);
  }
});

test('final receipt failure supersedes passing samples and is recorded', async () => {
  const {adapter} = fixture({verifyAllReceipts: async () => {throw Error('private-final-error');}});
  const saved = [];
  await assert.rejects(campaign(adapter, {count: 1, record: async packet => saved.push(packet)}),
    /final_receipt_verification_failed/);
  assert.equal(saved[0].verdict, 'RUNNING');
  assert.equal(saved.at(-1).verdict, 'FAIL');
  assert.equal(saved.at(-1).final_verification, 'FAIL');
  assert.equal(saved.at(-1).summary.passed, 1);
  assert(!JSON.stringify(saved).includes('private-final-error'));
});

test('evidence writer failure stops the campaign and cannot return success', async () => {
  const {adapter, calls} = fixture();
  await assert.rejects(campaign(adapter, {count: 2, record: async () => {throw Error('private-disk-error');}}),
    /evidence_write_failed/);
  assert.equal(calls.filter(name => name === 'wakeWithMessage').length, 1);
  assert(!calls.includes('verifyAllReceipts'));
});
