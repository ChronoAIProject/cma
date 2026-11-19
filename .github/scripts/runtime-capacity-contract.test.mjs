import assert from 'node:assert/strict';
import { test } from 'node:test';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { validateCapacity } from './runtime-capacity-contract.mjs';

const config = () => ({ streamLimit: 256, requestLimit: 32, streamReserve: 16,
  requestReserve: 8, nativeSessionReserve: 2, requestQueueLimit: 4, requestQueueMaxWaitMs: 1000,
  publicStreamLimit: 512, sharedRuntimeOutput: false, transport: 'kubernetes_exec', retainedProvider: false });
const emptyRetained = () => ({ activeAgents: 0, runtimeLinks: 0, publicObservers: 0,
  desktopStreams: 0, otherStreams: 0, streamLimit: 0, streamReserve: 0,
  requests: 0, requestLimit: 0, requestReserve: 0 });
function row(instance, active, observers) {
  return { instance, evidence: { source: 'synthetic', reference: 'local-fixture' },
    activeAgents: active, runtimeLinks: active, publicObservers: observers,
    desktopStreams: 2, otherStreams: 3, otherRequests: 1, streamPermits: active + 5,
    requestPermits: 13, queuedRequests: 1, queueMaxWaitMs: 200,
    requiredReserve: { streams: 8, requests: 4, publicStreams: 10, reference: 'synthetic-reserve' },
    pools: [{ role: 'native', limit: 16, reserve: 2, requiredReserve: 2,
      slots: 16, cachedConnections: 6, busy: 10, waiting: 2, ordinaryBusy: 10, otherConsumers: 0 }],
    retained: emptyRetained(),
    control: { operation: 'close_agent', samples: 20, successes: 20, p99Ms: 250, maxMs: 500, sameLaneBusy: true, reference: 'synthetic-controls' } };
}
function fixture() {
  const ids = ['instance-a', 'instance-b', 'instance-c'];
  return { version: 1, kind: 'synthetic', binding: { sourceRevision: 'synthetic-source',
    agentManagerImage: 'synthetic-manager', runtimeImage: 'synthetic-runtime',
    windowStart: '2026-01-01T00:00:00Z', windowEnd: '2026-01-01T01:00:00Z' },
    targets: { activeAgents: 200, publicObservers: 300 },
    instances: ids.map((id) => ({ id, config: config() })),
    scenarios: [
      { lostInstance: null, placement: [row(ids[0], 100, 150), row(ids[1], 60, 100), row(ids[2], 40, 50)] },
      ...ids.map((lostInstance) => ({ lostInstance, placement: ids.filter((id) => id !== lostInstance)
        .map((id, index) => row(id, index === 0 ? 140 : 60, index === 0 ? 220 : 80)) })),
    ] };
}
const eachRow = (input, action) => input.scenarios.forEach((scenario) => scenario.placement.forEach(action));
const changed = (edit) => { const input = fixture(); edit(input); return validateCapacity(input); };
const fails = (edit, pattern) => {
  const result = changed(edit);
  assert.equal(result.status, 'FAIL', JSON.stringify(result));
  assert.equal(result.qualification, 'NOT_ASSESSED');
  if (pattern) assert.match(result.reason, pattern);
};
const blocks = (edit, pattern) => {
  const result = changed(edit);
  assert.equal(result.status, 'BLOCKED', JSON.stringify(result));
  assert.equal(result.qualification, 'NOT_ASSESSED');
  if (pattern) assert.match(result.reason, pattern);
};

test('unequal surviving placement passes only the offline contract', () => {
  assert.deepEqual(validateCapacity(fixture()), { status: 'PASS', scope: 'offline_mapping_contract',
    qualification: 'NOT_ASSESSED', kind: 'synthetic', maximumSurvivingInstance: {
      streams: 145, requests: 13, nativeBusy: 10, nativeWaiting: 2 },
    maximumSurvivingPool: { native: { slots: 16, busy: 10, waiting: 2, otherConsumers: 0, requiredReserve: 2 } } });
});
test('public observers are not runtime links', () => {
  const input = fixture();
  input.targets.publicObservers = 600;
  eachRow(input, (value) => { value.publicObservers *= 2; });
  assert.equal(validateCapacity(input).status, 'PASS');
  input.scenarios[1].placement[0].streamPermits += 220;
  assert.match(validateCapacity(input).reason, /mapping mismatch/);
});
test('sharing off and on consume the same measured runtime permits', () => {
  for (const enabled of [false, true]) {
    const input = fixture();
    input.instances.forEach((instance) => { instance.config.sharedRuntimeOutput = enabled; });
    assert.equal(validateCapacity(input).status, 'PASS');
    input.scenarios[1].placement[0].streamPermits /= 2;
    assert.equal(validateCapacity(input).status, 'FAIL');
  }
});
test('same-lane waiters count against requests but do not multiply physical sessions', () => {
  assert.equal(validateCapacity(fixture()).status, 'PASS');
  fails((input) => { input.scenarios[1].placement[0].requestPermits -= 2; }, /request mapping/);
});
test('cached idle sockets do not consume request permits', () => {
  fails((input) => { input.scenarios[1].placement[0].requestPermits += 6; }, /request mapping/);
});
test('missing measured inputs and reserves remain blocked', () => {
  for (const name of ['runtimeLinks', 'desktopStreams', 'otherStreams', 'otherRequests', 'streamPermits', 'requestPermits']) {
    blocks((input) => { delete input.scenarios[1].placement[0][name]; }, /missing/);
  }
  blocks((input) => { delete input.scenarios[1].placement[0].requiredReserve; }, /missing/);
  blocks((input) => { delete input.scenarios[1].placement[0].pools[0].requiredReserve; }, /missing/);
  blocks((input) => { input.scenarios[1].placement[0].requiredReserve.reference = ''; }, /missing/);
});
test('all configured settings must be known, including disabled values', () => {
  for (const name of Object.keys(config())) {
    const result = changed((input) => { delete input.instances[0].config[name]; });
    assert.notEqual(result.status, 'PASS', name);
  }
});
test('retained and other consumers cannot disappear from inventory', () => {
  fails((input) => { input.instances[0].config.retainedProvider = true; }, /inventory/);
  fails((input) => { input.scenarios[1].placement[0].retained.runtimeLinks = 1; }, /unconfigured/);
  fails((input) => { input.scenarios[1].placement[0].otherStreams += 1; }, /mapping/);
  fails((input) => { input.scenarios[1].placement[0].otherRequests += 1; }, /mapping/);
});
test('retained provider uses its own pool and capacity observations', () => {
  const input = fixture();
  input.instances.forEach((instance) => { instance.config.retainedProvider = true; });
  eachRow(input, (value) => {
    value.activeAgents -= 5; value.runtimeLinks -= 5; value.streamPermits -= 5; value.publicObservers -= 5;
    value.retained = { activeAgents: 5, runtimeLinks: 5, publicObservers: 5, desktopStreams: 1,
      otherStreams: 2, streamLimit: 16, streamReserve: 4, requests: 2, requestLimit: 8, requestReserve: 2 };
    value.pools.push({ role: 'retained_provider', limit: 8, reserve: 2, requiredReserve: 2,
      slots: 3, cachedConnections: 2, busy: 1, waiting: 1, ordinaryBusy: 1, otherConsumers: 0 });
  });
  assert.equal(validateCapacity(input).status, 'PASS');
  assert.deepEqual(validateCapacity(input).maximumSurvivingPool.retained_provider,
    { slots: 3, busy: 1, waiting: 1, otherConsumers: 0, requiredReserve: 2 });
  input.scenarios[2].placement[0].retained.requests = 7;
  assert.equal(validateCapacity(input).status, 'FAIL');
});
test('a replica sum cannot conceal an overloaded surviving instance', () => {
  fails((input) => { input.instances[1].config.streamLimit = 128; }, /exceeds limit/);
  fails((input) => { input.instances[1].config.publicStreamLimit = 200; }, /exceeds limit/);
});
test('every one-instance loss must have exact survivor placement', () => {
  fails((input) => { input.scenarios.pop(); }, /list size/);
  blocks((input) => { input.scenarios[1].lostInstance = 'unknown'; }, /loss missing/);
  fails((input) => { input.scenarios[1].placement[0].instance = 'instance-a'; }, /lost instance/);
  fails((input) => { input.scenarios[1].placement[1].instance = input.scenarios[1].placement[0].instance; }, /duplicate/);
  fails((input) => { input.instances[1].id = input.instances[0].id; }, /duplicate/);
  fails((input) => { input.scenarios[1].lostInstance = null; }, /duplicate/);
});
test('targets cannot be reduced or inflated with idle observers', () => {
  fails((input) => { input.targets.activeAgents = 199; });
  fails((input) => { input.targets.publicObservers = 299; });
  fails((input) => { input.scenarios[1].placement[0].activeAgents = 0; }, /not fully placed/);
  fails((input) => { input.scenarios[1].placement[0].runtimeLinks = 0; }, /lacks runtime links/);
  fails((input) => { input.scenarios[1].placement[0].publicObservers -= 1; }, /not fully placed/);
});
test('pool reserve and physical exhaustion cannot hide behind process headroom', () => {
  fails((input) => { input.scenarios[1].placement[0].pools[0].requiredReserve = 3; }, /reserve unavailable/);
  fails((input) => { input.scenarios[1].placement[0].pools[0].slots = 17; }, /exceeds limit/);
  fails((input) => { input.scenarios[1].placement[0].pools[0].otherConsumers = 5; }, /exceeds limit/);
  fails((input) => { input.scenarios[1].placement[0].pools[0].limit = 32; }, /exact config/);
  fails((input) => { input.scenarios[1].placement[0].pools[0].reserve = 3; }, /exact config/);
});
test('contradictory occupancy, queue or reserve observations fail', () => {
  fails((input) => { input.scenarios[1].placement[0].pools[0].cachedConnections = 7; }, /inconsistent/);
  fails((input) => { input.scenarios[1].placement[0].pools[0].ordinaryBusy = 11; }, /inconsistent/);
  fails((input) => { input.scenarios[1].placement[0].queuedRequests = 5; }, /queue/);
  fails((input) => { input.scenarios[1].placement[0].queueMaxWaitMs = 1001; }, /queueWait/);
  fails((input) => { input.scenarios[1].placement[0].requiredReserve.streams = 17; }, /reserve/);
  fails((input) => { input.scenarios[1].placement[0].requiredReserve.requests = 9; }, /reserve/);
});
test('same-lane timeout characterization cannot become a control latency pass', () => {
  blocks((input) => { input.scenarios[1].placement[0].control.operation = 'resources'; }, /CloseAgent evidence/);
  blocks((input) => { input.scenarios[1].placement[0].control.samples = 0; }, /evidence missing/);
  blocks((input) => { input.scenarios[1].placement[0].control.sameLaneBusy = false; }, /not measured/);
  blocks((input) => { delete input.scenarios[1].placement[0].control; });
  fails((input) => { input.scenarios[1].placement[0].control.successes = 19; }, /controls failed/);
  fails((input) => { Object.assign(input.scenarios[1].placement[0].control, { p99Ms: 1001, maxMs: 2000 }); }, /controls failed/);
});
test('private candidate observations remain unqualified even when internally consistent', () => {
  const input = fixture();
  input.kind = 'candidate';
  Object.assign(input.binding, { sourceRevision: 'a'.repeat(40), agentManagerImage: `sha256:${'b'.repeat(64)}`, runtimeImage: `sha256:${'c'.repeat(64)}` });
  eachRow(input, (value) => { value.evidence.source = 'measured'; });
  const result = validateCapacity(input);
  assert.equal(result.status, 'PASS');
  assert.equal(result.qualification, 'NOT_ASSESSED');
  input.binding.runtimeImage = 'mutable-tag';
  assert.equal(validateCapacity(input).status, 'FAIL');
});
test('false qualification claims and unknown schema are refused', () => {
  for (const name of ['qualification', 'deployedCapacity', 'status']) fails((input) => { input[name] = 'PASS'; }, /unknown field/);
  fails((input) => { input.scenarios[1].placement[0].qualification = 'PASS'; }, /unknown field/);
  fails((input) => { input.scenarios[1].placement[0].pools[0].role = 'runtime-owner-agent-id'; }, /pool role/);
  fails((input) => { input.scenarios[1].placement[0].evidence.source = 'measured'; }, /wrong observation source/);
});
test('malformed, fractional, negative, unsafe and unknown numeric inputs fail closed', () => {
  for (const value of [-1, 0.5, '140', NaN, Infinity, Number.MAX_SAFE_INTEGER + 1, {}, []]) {
    fails((input) => { input.scenarios[1].placement[0].runtimeLinks = value; });
  }
  for (const value of [undefined, null]) blocks((input) => { input.scenarios[1].placement[0].runtimeLinks = value; });
  for (const value of [null, [], false, {}, { version: 2 }]) assert.notEqual(validateCapacity(value).status, 'PASS');
  fails((input) => { input.binding.windowEnd = input.binding.windowStart; }, /window/);
});
test('CLI reads only the supplied file and returns bounded failures', () => {
  const directory = mkdtempSync(join(tmpdir(), 'capacity-contract-'));
  const path = join(directory, 'observations.json');
  const command = fileURLToPath(new URL('./runtime-capacity-contract.mjs', import.meta.url));
  try {
    writeFileSync(path, JSON.stringify(fixture()));
    let result = spawnSync(process.execPath, [command, path], { encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(JSON.parse(result.stdout).qualification, 'NOT_ASSESSED');
    writeFileSync(path, '{private-invalid-json');
    result = spawnSync(process.execPath, [command, path], { encoding: 'utf8' });
    assert.equal(result.status, 1);
    assert(!result.stderr.includes('private-invalid-json'));
    result = spawnSync(process.execPath, [command], { encoding: 'utf8' });
    assert.equal(result.status, 1);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
