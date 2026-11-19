import test from 'node:test';
import assert from 'node:assert/strict';
import {cells} from './cells.mjs';
import {runSec01, validateRuntimeBinding, validateObservation, probeSpecs} from './runner.mjs';
import {OPTIONS, baseBinding, passingFixture, observation} from './test-support.mjs';
import {validServingReceipt} from './binding.mjs';
import {receipt} from './test-support.mjs';

const cell = cells[0];
const bind = () => baseBinding(cell, 'audit');
const candidate = {source: OPTIONS.source, image: OPTIONS.image,
  runtimeSource: OPTIONS.runtimeSource, runtimeImage: OPTIONS.runtimeImage};

test('standalone serving receipts require frozen process attribution around the request', () => {
  for (const am of ['a', 'b']) {
    for (const origin of ['http://127.0.0.1:4100', 'http://localhost:4100', 'http://[::1]:4100']) {
      const value = receipt(am);
      value.serving.origin = value.serving_after.origin = origin;
      assert.equal(validServingReceipt(value), true);
    }
  }
  for (const value of [undefined, null, true, {healthy: true}, {am: 'a', verdict: 'PASS'}]) {
    assert.equal(validServingReceipt(value), false);
  }
  const changes = [
    r => r.am = 'other', r => delete r.serving, r => delete r.serving_after,
    r => r.serving.schema = 'unknown', r => r.serving_after.schema = 'unknown',
    r => r.serving.am = 'b', r => r.serving_after.am = 'b',
    r => r.serving.ready = false, r => r.serving_after.ready = false,
    r => r.serving.draining = true, r => r.serving_after.draining = true,
    r => delete r.attribution, r => r.attribution.verdict = 'UNPROVED',
    r => r.attribution.basis = 'healthy_response', r => r.request_id = ' ',
    r => r.request_id = 1, r => r.started_at = 'invalid', r => r.ended_at = null,
    r => r.serving.observed_at = '2030-01-01T00:00:00Z',
    r => r.started_at = '2030-01-01T00:00:00Z',
    r => r.ended_at = '2030-01-01T00:00:00Z',
  ];
  for (const key of ['pod_uid', 'instance_id', 'container_id', 'origin']) {
    changes.push(r => r.serving[key] = '', r => r.serving_after[key] = 'changed');
  }
  for (const origin of ['invalid', 'https://localhost', 'http://remote.invalid',
    'http://user@localhost', 'http://localhost/path', 'http://localhost/?query', 'http://localhost/#fragment']) {
    changes.push(r => r.serving.origin = r.serving_after.origin = origin);
  }
  for (const change of changes) {
    const value = receipt('a'); change(value);
    assert.equal(validServingReceipt(value), false);
  }
});

for (const [name, change, message] of [
  ['wrong AM image', b => b.ams.a.image = '@sha256:' + 'd'.repeat(64), /two_am_binding/],
  ['identical tenant subjects', b => b.tenants.b.subject_sha256 = b.tenants.a.subject_sha256, /independent_tenant/],
  ['identical tenant organizations', b => b.tenants.b.organization_sha256 = b.tenants.a.organization_sha256, /independent_tenant/],
  ['label-only AM receipts', b => b.serving = [{am: 'a', verdict: 'PASS'}, {am: 'b', verdict: 'PASS'}], /serving/],
  ['one AM only', b => b.serving = [b.serving[0]], /serving/],
  ['wrong serving process', b => b.serving[1].serving.instance_id = 'another-process', /serving/],
  ['AM changes during request', b => b.serving[1].serving_after.pod_uid = 'new-pod', /serving/],
  ['missing request ID', b => b.serving[1].request_id = '', /serving/],
  ['out-of-order timing', b => b.serving[1].started_at = '2030-01-01', /serving/],
  ['wrong source', b => b.source_sha = 'd'.repeat(40), /candidate/],
  ['wrong runtime image', b => b.runtime_image = '@sha256:' + 'd'.repeat(64), /runtime_image/],
  ['missing volume', b => delete b.volume_sha256, /runtime_image/],
  ['wrong state', b => b.state = 'resumed', /context/],
  ['not genuinely cold', b => b.lifecycle.tenant_history = true, /not_new/],
  ['missing cold receipt', b => b.lifecycle.transitions = [], /order/],
]) test(name + ' blocks the runtime binding', () => {
  const b = bind(); change(b);
  assert.throws(() => validateRuntimeBinding(cell, b, 'audit', candidate), message);
});

test('both supported warm pool flows require actual ordered transitions', () => {
  const warm = cells.find(x => x.state === 'warm');
  const b = baseBinding(warm, 'audit');
  validateRuntimeBinding(warm, b, 'audit', candidate);
  b.lifecycle.reused_after_retirement = false;
  assert.throws(() => validateRuntimeBinding(warm, b, 'audit', candidate), /warm_pool/);
  b.lifecycle.pool_claimed = true; b.lifecycle.used_cross_tenant_reuse_refused = true;
  b.lifecycle.transitions[1].action = 'reuse_refused';
  validateRuntimeBinding(warm, b, 'audit', candidate);
  b.lifecycle.transitions[1].sequence = b.lifecycle.transitions[0].sequence;
  assert.throws(() => validateRuntimeBinding(warm, b, 'audit', candidate), /order/);
});

test('resume must preserve own data and revoke while paused', () => {
  const resumed = cells.find(x => x.state === 'resumed');
  for (const change of [b => b.lifecycle.own_data_after_sha256 = 'd'.repeat(64),
    b => b.lifecycle.authorization_changed_while_paused = false,
    b => b.lifecycle.transitions.reverse()]) {
    const b = baseBinding(resumed, 'audit'); change(b);
    assert.throws(() => validateRuntimeBinding(resumed, b, 'audit', candidate));
  }
});

test('per-probe receipts must match exact ingress, nonce, tenant and incarnation', () => {
  const b = bind(), spec = probeSpecs(cell, b)[0];
  const ctx = {am: 'b', run_id: 'audit', runtime_id: b.runtime_id, nonce: b.nonce,
    owner_tenant: cell.owner_tenant, foreign_tenant: cell.foreign_tenant};
  for (const change of [o => o.am = 'a', o => o.nonce = 'replayed-nonce',
    o => o.tenant.foreign = cell.owner_tenant, o => o.incarnation = 'different-runtime',
    o => delete o.serving_receipt]) {
    const o = observation(spec, b, ctx); change(o);
    assert.throws(() => validateObservation(o, spec, b, ctx));
  }
});

test('the harness enforces explicit local parent lease and pinned candidate before setup', async () => {
  for (const change of [o => delete o.source, o => delete o.runtimeImage,
    o => delete o.record, o => o.lease = {...o.lease, single_matrix_writer: false},
    o => o.lease = {...o.lease, scope: 'PRODUCTION'}, o => o.repetitions = 0]) {
    let called = 0;
    const options = {...OPTIONS, fixture: {prepareCell() {called++;}}}; change(options);
    await assert.rejects(runSec01(options)); assert.equal(called, 0);
  }
});

test('each probe executes on both AMs and final evidence records cleanup', async () => {
  const events = [], fixture = passingFixture();
  const packet = await runSec01({...OPTIONS, fixture,
    record: async event => events.push(structuredClone(event))});
  assert.equal(packet.verdict, 'PASS');
  for (const result of packet.results) {
    const specs = probeSpecs(cells.find(x => x.id === result.id), {});
    assert.equal(result.attempts, specs.length * 2);
    for (const spec of specs) assert.deepEqual(result.outcomes.filter(x => x.id === spec.id).map(x => x.am), ['a', 'b']);
    assert.equal(result.cleanup, 'PASS');
  }
  const recorded = events.filter(x => x.kind === 'sec01_cell_final');
  assert.equal(recorded.length, 30);
  assert.ok(recorded.every(x => x.result.cleanup === 'PASS'));
  assert.equal(events.filter(x => x.kind === 'sec01_probe_observation').length,
    packet.results.reduce((n, x) => n + x.attempts, 0));
});

test('evidence writer failures still run cleanup and never pass', async () => {
  for (const kind of ['sec01_probe_observation', 'sec01_cell_final', 'sec01_run_final']) {
    let cleanup = 0;
    const fixture = passingFixture();
    fixture.prepareCell = async ({cell, run_id}) => ({...baseBinding(cell, run_id), cleanup: async () => {cleanup++;}});
    const packet = await runSec01({...OPTIONS, fixture, record: async event => {
      if (event.kind === kind) throw Error('disk_full');
    }});
    assert.equal(packet.verdict, 'FAIL'); assert.equal(cleanup, 30);
  }
});

test('binding failure and cancellation both preserve cleanup custody', async () => {
  let cleanup = 0, probes = 0;
  const fixture = passingFixture();
  fixture.prepareCell = async ({cell, run_id}) => ({...baseBinding(cell, run_id), source_sha: 'wrong',
    cleanup: async () => {cleanup++;}});
  fixture.runtimeProbe = async () => {probes++;};
  const packet = await runSec01({...OPTIONS, fixture});
  assert.equal(packet.verdict, 'BLOCKED'); assert.equal(cleanup, 30); assert.equal(probes, 0);
  const cancelled = await runSec01({...OPTIONS, fixture, signal: AbortSignal.abort()});
  assert.equal(cancelled.results.length, 30); assert.equal(cancelled.verdict, 'BLOCKED');
  assert.equal(cleanup, 30);
});

test('an observed escape survives missing receipt evidence and subsequent repetitions', async () => {
  const fixture = passingFixture(), probe = fixture.runtimeProbe;
  fixture.runtimeProbe = async args => {
    const o = await probe(args);
    if (args.cell.repetition === 1 && args.spec.kind === 'tcp') {
      o.addresses[0].connected = true; delete o.serving_receipt;
    }
    return o;
  };
  const packet = await runSec01({...OPTIONS, fixture, repetitions: 2});
  assert.equal(packet.verdict, 'FAIL');
  assert.equal(packet.counts[cell.id].FAIL, 1); assert.equal(packet.counts[cell.id].PASS, 1);
});

test('raw provider error bodies never enter recorded evidence', async () => {
  const fixture = passingFixture(), probe = fixture.runtimeProbe, events = [];
  fixture.runtimeProbe = async args => ({...await probe(args), body: 'private-provider-error',
    Authorization: 'private-provider-header', arbitrary: {token: 'private-provider-token'}});
  const result = await runSec01({...OPTIONS, fixture, record: async e => events.push(e)});
  assert.equal(result.verdict, 'PASS');
  assert.equal(JSON.stringify(events).includes('private-provider-'), false);
});
