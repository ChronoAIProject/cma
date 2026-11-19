#!/usr/bin/env node
// Offline mapping checks only. Input observations and evidence references are
// assertions by the caller; this program cannot qualify a deployed candidate.
// Supply one baseline and one placement for every lost instance. Each placement
// describes concurrent demand, not sums of unrelated metric peaks. Pool roles
// are unique within a process; otherConsumers is additional background demand
// excluded from busy. Reserve references explain the separately required room.
// Unclassified native demand is ordinary. Only a complete, evidenced breakdown
// of effective Continuity/Control demand may consume configured process reserve.
// The synthetic test fixture is the executable input example. Keep candidate
// observations outside the public repository.
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

class ContractError extends Error {
  constructor(status, message) { super(message); this.status = status; }
}
const fail = (message) => { throw new ContractError('FAIL', message); };
const blocked = (message) => { throw new ContractError('BLOCKED', message); };
function object(value, path) {
  if (value === null || value === undefined) blocked(`${path}: observation missing`);
  if (typeof value !== 'object' || Array.isArray(value)) fail(`${path}: expected object`);
  return value;
}
function fields(value, names, path) {
  object(value, path);
  for (const name of Object.keys(value)) if (!names.includes(name)) fail(`${path}: unknown field ${name}`);
}
function number(value, path, min = 0, max = Number.MAX_SAFE_INTEGER) {
  if (value === null || value === undefined) blocked(`${path}: measured value missing`);
  if (!Number.isSafeInteger(value) || value < min || value > max) fail(`${path}: invalid integer`);
  return value;
}
function text(value, path) {
  if (value === null || value === undefined || value === '') blocked(`${path}: evidence missing`);
  if (typeof value !== 'string' || value.length > 512) fail(`${path}: invalid text`);
  return value;
}
function boolean(value, path) {
  if (value === null || value === undefined) blocked(`${path}: setting missing`);
  if (typeof value !== 'boolean') fail(`${path}: expected boolean`);
}
function array(value, path, min = 1, max = 128) {
  if (value === null || value === undefined) blocked(`${path}: observations missing`);
  if (!Array.isArray(value) || value.length < min || value.length > max) fail(`${path}: invalid list size`);
  return value;
}
function unique(values, path) {
  if (new Set(values).size !== values.length) fail(`${path}: duplicate entry`);
}
function within(demand, reserve, limit, path) {
  if (!Number.isSafeInteger(demand + reserve) || demand + reserve > limit) fail(`${path}: demand plus reserve exceeds limit`);
}
function evidence(value, kind, path) {
  fields(value, ['source', 'reference'], path);
  if (value.source !== (kind === 'candidate' ? 'measured' : 'synthetic')) fail(`${path}: wrong observation source`);
  text(value.reference, `${path}.reference`);
}
function configuration(config, path) {
  fields(config, ['streamLimit', 'requestLimit', 'streamReserve', 'requestReserve',
    'nativeSessionReserve', 'requestQueueLimit', 'requestQueueMaxWaitMs',
    'publicStreamLimit', 'sharedRuntimeOutput', 'transport', 'retainedProvider'], path);
  for (const name of ['streamLimit', 'requestLimit']) number(config[name], `${path}.${name}`, 1, 4096);
  for (const kind of ['stream', 'request']) number(config[`${kind}Reserve`], `${path}.${kind}Reserve`, 0, config[`${kind}Limit`] - 1);
  number(config.nativeSessionReserve, `${path}.nativeSessionReserve`, 0, 15);
  number(config.requestQueueLimit, `${path}.requestQueueLimit`, 0, 1024);
  number(config.requestQueueMaxWaitMs, `${path}.requestQueueMaxWaitMs`, 1, 60000);
  number(config.publicStreamLimit, `${path}.publicStreamLimit`, 1);
  boolean(config.sharedRuntimeOutput, `${path}.sharedRuntimeOutput`);
  boolean(config.retainedProvider, `${path}.retainedProvider`);
  if (!['kubernetes_exec', 'kubernetes_portforward'].includes(config.transport)) fail(`${path}: unsupported native transport mapping`);
}
function pool(value, config, path) {
  fields(value, ['role', 'limit', 'reserve', 'requiredReserve', 'slots', 'cachedConnections',
    'busy', 'waiting', 'ordinaryBusy', 'otherConsumers'], path);
  if (!['native', 'retained_provider'].includes(value.role)) fail(`${path}: unknown pool role`);
  for (const name of ['limit', 'reserve', 'requiredReserve', 'slots', 'cachedConnections', 'busy', 'waiting', 'ordinaryBusy', 'otherConsumers']) {
    number(value[name], `${path}.${name}`, name === 'limit' ? 1 : 0);
  }
  if (value.role === 'native' && (value.limit !== 16 || value.reserve !== config.nativeSessionReserve)) fail(`${path}: native pool differs from exact config`);
  if (value.reserve >= value.limit || value.reserve < value.requiredReserve) fail(`${path}: reserve unavailable`);
  if (value.busy + value.cachedConnections > value.slots || value.ordinaryBusy > value.busy) fail(`${path}: inconsistent session occupancy`);
  within(value.slots, 0, value.limit, path);
  // Idle entries can be evicted. Waiters retain a request permit but share their
  // lane's busy session; adding them to physical slots would double count it.
  within(value.ordinaryBusy + value.otherConsumers, value.reserve, value.limit, `${path}.ordinary`);
  within(value.busy + value.otherConsumers, value.requiredReserve, value.limit, `${path}.headroom`);
}
function control(value, path) {
  fields(value, ['operation', 'samples', 'successes', 'p99Ms', 'maxMs', 'sameLaneBusy', 'reference'], path);
  if (value.operation !== 'close_agent') blocked(`${path}: same-lane CloseAgent evidence missing`);
  number(value.samples, `${path}.samples`);
  if (value.samples === 0) blocked(`${path}: successful same-lane control evidence missing`);
  number(value.successes, `${path}.successes`, 0, value.samples);
  number(value.p99Ms, `${path}.p99Ms`);
  number(value.maxMs, `${path}.maxMs`, value.p99Ms);
  if (value.sameLaneBusy !== true) blocked(`${path}: busy same-lane control was not measured`);
  text(value.reference, `${path}.reference`);
  if (value.successes !== value.samples || value.p99Ms > 1000) fail(`${path}: same-lane controls failed or exceeded one-second p99`);
}
function ordinaryDemand(row, native, kind, path) {
  const totals = { runtimeLinks: row.runtimeLinks, desktopStreams: row.desktopStreams,
    otherStreams: row.otherStreams, nativeBusy: native.busy,
    nativeWaiting: native.waiting, otherRequests: row.otherRequests };
  const ordinary = { ...totals };
  if (row.privileged !== undefined) {
    const privileged = row.privileged;
    fields(privileged, ['boundary', 'evidence', ...Object.keys(totals)], `${path}.privileged`);
    if (privileged.boundary !== 'shared_native_process') fail(`${path}: privileged demand has a different accounting boundary`);
    evidence(privileged.evidence, kind, `${path}.privileged.evidence`);
    for (const [name, total] of Object.entries(totals)) {
      number(privileged[name], `${path}.privileged.${name}`, 0, total);
      ordinary[name] -= privileged[name];
    }
    if (ordinary.nativeBusy !== native.ordinaryBusy) fail(`${path}: privileged classification contradicts native ordinaryBusy`);
  }
  return { streams: ordinary.runtimeLinks + ordinary.desktopStreams + ordinary.otherStreams,
    requests: ordinary.nativeBusy + ordinary.nativeWaiting + ordinary.otherRequests };
}
function placement(row, instance, kind, path) {
  fields(row, ['instance', 'evidence', 'activeAgents', 'runtimeLinks', 'publicObservers',
    'desktopStreams', 'otherStreams', 'otherRequests', 'streamPermits', 'requestPermits',
    'queuedRequests', 'queueMaxWaitMs', 'requiredReserve', 'pools', 'retained', 'control', 'privileged'], path);
  evidence(row.evidence, kind, `${path}.evidence`);
  const counts = ['activeAgents', 'runtimeLinks', 'publicObservers', 'desktopStreams',
    'otherStreams', 'otherRequests', 'streamPermits', 'requestPermits', 'queuedRequests', 'queueMaxWaitMs'];
  for (const name of counts) number(row[name], `${path}.${name}`);
  const config = instance.config;
  fields(row.requiredReserve, ['streams', 'requests', 'publicStreams', 'reference'], `${path}.requiredReserve`);
  for (const name of ['streams', 'requests', 'publicStreams']) number(row.requiredReserve[name], `${path}.requiredReserve.${name}`);
  text(row.requiredReserve.reference, `${path}.requiredReserve.reference`);
  if (row.runtimeLinks < row.activeAgents) fail(`${path}: active work lacks runtime links`);
  // sharedRuntimeOutput shares output representation, not native connections.
  // Public observers consume public admission, never an inferred runtime permit.
  if (row.streamPermits !== row.runtimeLinks + row.desktopStreams + row.otherStreams) fail(`${path}: runtime stream mapping mismatch`);
  if (config.streamReserve < row.requiredReserve.streams || config.requestReserve < row.requiredReserve.requests) fail(`${path}: process reserve unavailable`);
  within(row.streamPermits, row.requiredReserve.streams, config.streamLimit, `${path}.streams`);
  within(row.requestPermits, row.requiredReserve.requests, config.requestLimit, `${path}.requests`);
  within(row.queuedRequests, 0, config.requestQueueLimit, `${path}.queue`);
  within(row.queueMaxWaitMs, 0, config.requestQueueMaxWaitMs, `${path}.queueWait`);
  const pools = array(row.pools, `${path}.pools`, 1, 2);
  unique(pools.map((p) => object(p, `${path}.pool`).role), `${path}.pools`);
  for (const item of pools) pool(item, config, `${path}.pool.${item.role}`);
  const native = pools.find((p) => p.role === 'native');
  if (!native) blocked(`${path}: native pool observations missing`);
  if (row.requestPermits !== native.busy + native.waiting + row.otherRequests) fail(`${path}: process request mapping mismatch`);
  const ordinary = ordinaryDemand(row, native, kind, path);
  within(ordinary.streams, config.streamReserve, config.streamLimit, `${path}.ordinary streams`);
  within(ordinary.requests, config.requestReserve, config.requestLimit, `${path}.ordinary requests`);
  const retainedPool = pools.find((p) => p.role === 'retained_provider');
  if (Boolean(retainedPool) !== config.retainedProvider) fail(`${path}: retained pool inventory differs from config`);
  fields(row.retained, ['activeAgents', 'runtimeLinks', 'publicObservers', 'desktopStreams',
    'otherStreams', 'streamLimit', 'streamReserve', 'requests', 'requestLimit', 'requestReserve'], `${path}.retained`);
  for (const name of ['activeAgents', 'runtimeLinks', 'publicObservers', 'desktopStreams',
    'otherStreams', 'streamLimit', 'streamReserve', 'requests', 'requestLimit', 'requestReserve']) number(row.retained[name], `${path}.retained.${name}`);
  const retained = row.retained;
  if (!config.retainedProvider && Object.values(retained).some((n) => n !== 0)) fail(`${path}: unconfigured retained consumer`);
  if (retained.runtimeLinks < retained.activeAgents) fail(`${path}: retained active work lacks links`);
  within(retained.runtimeLinks + retained.desktopStreams + retained.otherStreams, retained.streamReserve, retained.streamLimit, `${path}.retainedStreams`);
  within(retained.requests, retained.requestReserve, retained.requestLimit, `${path}.retainedRequests`);
  if (retainedPool && retained.requests < retainedPool.busy + retainedPool.waiting) fail(`${path}: retained request mapping mismatch`);
  within(row.publicObservers + retained.publicObservers, row.requiredReserve.publicStreams, config.publicStreamLimit, `${path}.public`);
  control(row.control, `${path}.control`);
  return { active: row.activeAgents + retained.activeAgents, public: row.publicObservers + retained.publicObservers,
    streams: row.streamPermits, requests: row.requestPermits, nativeBusy: native.busy, nativeWaiting: native.waiting };
}

export function validateCapacity(input) {
  try {
    fields(input, ['version', 'kind', 'binding', 'targets', 'instances', 'scenarios'], 'input');
    if (input.version !== 1) fail('input: unsupported version');
    if (!['synthetic', 'candidate'].includes(input.kind)) fail('input: expected synthetic or candidate observations');
    fields(input.binding, ['sourceRevision', 'agentManagerImage', 'runtimeImage', 'windowStart', 'windowEnd'], 'binding');
    for (const name of ['sourceRevision', 'agentManagerImage', 'runtimeImage', 'windowStart', 'windowEnd']) text(input.binding[name], `binding.${name}`);
    if (input.kind === 'candidate') {
      if (!/^[a-f0-9]{40}$/.test(input.binding.sourceRevision)) fail('binding: invalid source revision');
      for (const name of ['agentManagerImage', 'runtimeImage']) if (!/^sha256:[a-f0-9]{64}$/.test(input.binding[name])) fail(`binding.${name}: digest required`);
    }
    const start = Date.parse(input.binding.windowStart), end = Date.parse(input.binding.windowEnd);
    if (!Number.isFinite(start) || !Number.isFinite(end) || start >= end) fail('binding: invalid observation window');
    fields(input.targets, ['activeAgents', 'publicObservers'], 'targets');
    number(input.targets.activeAgents, 'targets.activeAgents', 200);
    number(input.targets.publicObservers, 'targets.publicObservers', 300);
    const instances = array(input.instances, 'instances', 2, 64);
    for (const instance of instances) {
      fields(instance, ['id', 'config'], 'instance');
      text(instance.id, 'instance.id');
      configuration(instance.config, 'instance.config');
    }
    const ids = instances.map((i) => i.id);
    unique(ids, 'instances');
    const scenarios = array(input.scenarios, 'scenarios', instances.length + 1, instances.length + 1);
    unique(scenarios.map((s) => object(s, 'scenario').lostInstance), 'scenarios');
    for (const loss of [null, ...ids]) if (!scenarios.some((s) => s.lostInstance === loss)) blocked('scenarios: baseline or one-instance loss missing');
    const maxima = { streams: 0, requests: 0, nativeBusy: 0, nativeWaiting: 0 };
    const poolMaxima = {};
    for (const scenario of scenarios) {
      fields(scenario, ['lostInstance', 'placement'], 'scenario');
      const survivors = ids.filter((id) => id !== scenario.lostInstance);
      const rows = array(scenario.placement, 'scenario.placement', survivors.length, survivors.length);
      unique(rows.map((r) => object(r, 'placement').instance), 'placement');
      let active = 0, publicCount = 0;
      for (const row of rows) {
        if (!survivors.includes(row.instance)) fail('placement: unknown or lost instance');
        const values = placement(row, instances.find((i) => i.id === row.instance), input.kind, 'placement');
        active += values.active; publicCount += values.public;
        if (scenario.lostInstance !== null) {
          for (const key of Object.keys(maxima)) maxima[key] = Math.max(maxima[key], values[key]);
          for (const pool of row.pools) {
            const peak = poolMaxima[pool.role] ??= { slots: 0, busy: 0, waiting: 0, otherConsumers: 0, requiredReserve: 0 };
            for (const key of Object.keys(peak)) peak[key] = Math.max(peak[key], pool[key]);
          }
        }
      }
      if (active !== input.targets.activeAgents || publicCount !== input.targets.publicObservers) fail('placement: target workload not fully placed');
    }
    return { status: 'PASS', scope: 'offline_mapping_contract', qualification: 'NOT_ASSESSED',
      kind: input.kind, maximumSurvivingInstance: maxima, maximumSurvivingPool: poolMaxima };
  } catch (error) {
    if (!(error instanceof ContractError)) throw error;
    return { status: error.status, scope: 'offline_mapping_contract', qualification: 'NOT_ASSESSED', reason: error.message };
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (process.argv.length !== 3) fail('usage: node runtime-capacity-contract.mjs <observations.json>');
    const content = readFileSync(process.argv[2]);
    if (content.length > 4 * 1024 * 1024) fail('input: exceeds 4 MiB limit');
    const result = validateCapacity(JSON.parse(content.toString('utf8')));
    process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
    process.exitCode = result.status === 'PASS' ? 0 : 1;
  } catch {
    // Do not echo private paths, supplied values, or JSON fragments in failures.
    process.stderr.write('Capacity input could not be read or parsed; qualification NOT_ASSESSED.\n');
    process.exitCode = 1;
  }
}
