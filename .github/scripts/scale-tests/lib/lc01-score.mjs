import { check, hash, isHash, reconcile } from './checks.mjs';
import { activeWindow, end, loadPlan, protocols, steadyEnd, steadyStart, workloadShape } from './lc01-plan.mjs';
import { deliveryPlan } from './lc01-run.mjs';

const failure = (condition, list, code) => { if (!condition) list.push(code); };
const finite = (n) => typeof n === 'number' && Number.isFinite(n) && n >= 0;
const identity = (row) => `${row.user}/${row.at}`;
const readIdentity = (row) => `${row.kind}/${identity(row)}`;

const expected = loadPlan().filter((entry) => entry.kind === 'turn');

function positiveSamples(samples) {
  return Array.isArray(samples) && samples.length > 0 && samples.every(finite);
}

export function distribution(samples) {
  if (samples.length === 0) return { n: 0, max: null };
  check(positiveSamples(samples), 'invalid_latency_samples');
  const sorted = [...samples].sort((a, b) => a - b);
  const at = (p) => sorted[Math.ceil(p * sorted.length) - 1];
  return { n: sorted.length, max: sorted.at(-1), ...(sorted.length >= 1000
    ? { p50: at(0.5), p95: at(0.95), p99: at(0.99) } : {}) };
}

function groupSamples(samples, name, values) {
  if (!samples.has(name)) samples.set(name, []);
  const group = samples.get(name);
  for (const value of values) group.push(value);
}

function workload(rows, header, samples, failed, seenAdmissions) {
  const turns = new Map();
  const readSamples = new Map();
  for (const row of rows) {
    if (row.type === 'read') {
      check(!readSamples.has(readIdentity(row)) && finite(row.latencyMs), 'duplicate_or_invalid_read');
      readSamples.set(readIdentity(row), row);
      continue;
    }
    if (row.at === undefined) continue;
    if (!turns.has(identity(row))) turns.set(identity(row), new Map());
    const events = turns.get(identity(row));
    check(!events.has(row.type), 'duplicate_turn_observation');
    events.set(row.type, row);
  }
  const admitted = [];
  const counts = { offered: 0, admitted: 0, completed: 0, refused: 0, overdue: 0, unknown: 0 };
  const providerCounts = {};
  let requestFailures = 0;
  for (const entry of expected) {
    const events = turns.get(identity(entry));
    failure(events?.get('offered')?.protocol === entry.protocol, failed, 'scheduled_turn_missing');
    if (!events?.has('offered')) continue;
    counts.offered += 1;
    const receipt = events.get('admitted');
    const terminal = events.get('terminal');
    const user = header.users[entry.user];
    const stratum = `${entry.protocol}/${user.providerClass}/${user.modelClass}`;
    const steady = entry.at >= steadyStart && entry.at < steadyEnd;
    if (steady) providerCounts[stratum] ??= { offered: 0, admitted: 0, completed: 0 };
    if (steady) providerCounts[stratum].offered += 1;
    if (receipt) {
      check(isHash(receipt.commandHash) && isHash(receipt.responseHash) && isHash(receipt.keyHash),
        'invalid_admission_identity');
      for (const [field, seen] of Object.entries(seenAdmissions)) {
        check(!seen.has(receipt[field]), 'admission_identity_reused');
        seen.add(receipt[field]);
      }
      admitted.push(receipt);
      counts.admitted += 1;
      if (steady) providerCounts[stratum].admitted += 1;
      if (steady) groupSamples(samples, `ack/${stratum}`, [events.get('acknowledged')?.headersMs ?? receipt.headersMs]);
      if (terminal?.state === 'completed') {
        counts.completed += 1;
        if (steady) providerCounts[stratum].completed += 1;
      }
      failure(terminal?.responseHash === receipt.responseHash && terminal.state === 'completed'
        && finite(terminal.elapsedMs) && terminal.elapsedMs <= 120_000
        && events.get('file_verified')?.ok === true, failed, 'accepted_turn_not_verified');
      if (events.has('input_observation_failed')) requestFailures += 1;
    } else {
      if (events.has('refused')) counts.refused += 1;
      else if (events.has('overdue')) counts.overdue += 1;
      else counts.unknown += 1;
      requestFailures += 1;
    }
  }
  failure(turns.size === expected.length, failed, 'unexpected_turn_schedule');
  for (const protocol of protocols) {
    const group = Object.entries(providerCounts).filter(([label]) => label.startsWith(`${protocol}/`));
    const funded = group.filter(([label]) => label.startsWith(`${protocol}/funded/`))
      .reduce((n, [, count]) => n + count.admitted, 0);
    const total = group.reduce((n, [, count]) => n + count.admitted, 0);
    failure(total > 0 && funded >= total / 10, failed, 'funded_admissions_below_floor');
  }

  const requestCounts = new Map();
  function outcome(consumer, ok) {
    if (!requestCounts.has(consumer)) requestCounts.set(consumer, { attempted: 0, failed: 0 });
    const row = requestCounts.get(consumer);
    row.attempted += 1;
    if (!ok) row.failed += 1;
  }
  for (const entry of deliveryPlan(header)) {
    if (!['read', 'burst', 'background'].includes(entry.kind)) continue;
    const read = readSamples.get(readIdentity(entry));
    const consumer = entry.kind === 'background' ? header.background[entry.user].consumer : 'public';
    failure(read?.consumer === consumer, failed, 'scheduled_read_missing_or_wrong_consumer');
    outcome(consumer, read?.http === 200 && read.failure === null);
    if (read?.http === 200) groupSamples(samples, `read/${consumer}/${entry.kind}`, [read.latencyMs]);
    readSamples.delete(readIdentity(entry));
  }
  failure(readSamples.size === 0, failed, 'unexpected_read_schedule');
  for (const stats of requestCounts.values()) {
    failure(stats.attempted > 0 && stats.failed / stats.attempted < 0.001, failed, 'request_failure_budget_exceeded');
  }
  failure(counts.offered > 0 && requestFailures / counts.offered < 0.001, failed, 'turn_request_failure_budget_exceeded');
  return { counts, providerCounts, requests: Object.fromEntries(requestCounts), admitted };
}

function streams(rows, samples, failed) {
  const states = Array.from({ length: 50 }, () => ({ open: null, intervals: [], cursor: null, churn: 0 }));
  const eventHashes = [];
  for (const row of rows.filter((row) => row.type.startsWith('stream_'))) {
    const state = states[row.user];
    check(state && finite(row.ms) && /^(0|[1-9]\d{0,19})$/.test(row.cursor), 'invalid_stream_observation');
    if (row.type === 'stream_attempt') {
      failure(state.cursor === null || state.cursor === row.cursor, failed, 'resume_cursor_changed');
      state.cursor = row.cursor;
    } else if (row.type === 'stream_open') {
      check(state.open === null && state.cursor === row.cursor, 'invalid_stream_open');
      state.open = row.ms;
      groupSamples(samples, 'stream/public/agent-events', [row.headersMs]);
    } else if (row.type === 'stream_cursor') {
      failure(state.open !== null && BigInt(row.cursor) > BigInt(state.cursor) && isHash(row.eventHash),
        failed, 'sequence_regression');
      state.cursor = row.cursor;
      eventHashes.push(hash(`${row.user}/${row.cursor}/${row.eventHash}`));
    } else if (row.type === 'stream_close') {
      if (state.open !== null) state.intervals.push([state.open, row.ms]);
      if (row.reason !== 'scheduled') state.churn += 1;
      state.open = null;
    }
  }
  for (const [user, state] of states.entries()) {
    const { open, close } = activeWindow(user);
    failure(state.open === null && state.intervals.length > 0 && state.intervals[0][0] - open <= 2000
      && state.intervals.at(-1)[1] >= close, failed, 'observer_window_incomplete');
    // Keep all outage time in the stream denominator.
    const steadyConnected = state.intervals.reduce((total, [from, to]) => total
      + Math.max(0, Math.min(to, steadyEnd) - Math.max(from, steadyStart)), 0);
    failure(steadyConnected / (steadyEnd - steadyStart) >= 0.999, failed, 'stream_availability_below_floor');
  }
  return { churn: states.reduce((total, row) => total + row.churn, 0), eventHashes };
}

function inventoryMatches(expected, observed) {
  if (!Array.isArray(expected) || expected.length === 0 || !expected.every(isHash)) return false;
  const members = new Set(expected);
  return members.size === expected.length && observed.length === members.size
    && observed.every((identity) => members.delete(identity));
}

function checkInfrastructure(evidence, samples, failed) {
  const bounded = (samples, limit, percentile = 1, inclusive = true) => {
    check(positiveSamples(samples), 'infrastructure_samples_missing');
    const sorted = [...samples].sort((a, b) => a - b);
    const observed = sorted[Math.ceil(sorted.length * percentile) - 1];
    failure(inclusive ? observed <= limit : observed < limit, failed, 'infrastructure_threshold_exceeded');
  };
  check(Array.isArray(evidence.am) && evidence.am.length > 0 && isHash(evidence.resourcesHash),
    'resource_evidence_missing');
  const inventory = evidence.memberInventory;
  check(inventory?.independent === true && inventory.run === evidence.run && inventory.startUtc === evidence.startUtc
    && inventory.endUtc === evidence.endUtc && inventory.sourceSha === evidence.sourceSha
    && inventory.imageDigest === evidence.imageDigest && isHash(inventory.evidenceHash),
  'resource_inventory_window_mismatch');
  check(inventoryMatches(inventory.amReplicaHashes, evidence.am.map((row) => row.replicaHash)),
    'am_replica_inventory_mismatch');
  for (const replica of evidence.am) {
    bounded(replica.idleCpu, 0.40);
    bounded(replica.steadyCpu, 0.60, 0.95);
    bounded(replica.burstCpu, 0.75, 0.95);
    bounded(replica.memory, 0.70);
    bounded(replica.throttle, 0.01, 1, false);
  }
  const mongo = evidence.mongo;
  check(mongo && Array.isArray(mongo.servers) && mongo.servers.length > 0 && positiveSamples(mongo.atlasHeadroom),
    'mongo_evidence_missing');
  check(inventoryMatches(inventory.mongoServerHashes, mongo.servers.map((row) => row.serverHash)),
    'mongo_server_inventory_mismatch');
  failure(Math.min(...mongo.atlasHeadroom) >= 0.30, failed, 'atlas_headroom_below_floor');
  for (const server of mongo.servers) {
    check(positiveSamples(server.checkoutMs), 'mongo_checkout_evidence_missing');
    bounded(server.checkoutMs, 1000, 1, false);
    bounded(server.checkoutMs, 25, 0.95, false);
    if (server.checkoutMs.length >= 1000) {
      bounded(server.checkoutMs, 100, 0.99, false);
    }
    groupSamples(samples, `mongo/checkout/${server.serverHash}`, server.checkoutMs);
    bounded(server.occupancy, 0.70, 0.95);
  }
  check(Array.isArray(evidence.egressPools) && evidence.egressPools.length > 0
    && isHash(evidence.nPlusOneHash), 'placement_evidence_missing');
  check(inventoryMatches(inventory.egressPoolHashes, evidence.egressPools.map((row) => row.poolHash)),
    'egress_pool_inventory_mismatch');
  for (const pool of evidence.egressPools) {
    bounded(pool.cpuWithPending, 0.70);
    bounded(pool.memoryWithPending, 0.70);
    failure(pool.largestNodeLossPlaceable === true, failed, 'n_plus_one_not_placeable');
  }
}

export function scoreCapacity(captures) {
  check(Array.isArray(captures) && captures.length > 0, 'capture_required', true);
  const samples = new Map();
  const failed = [];
  const blocked = [];
  const runs = [];
  const runIds = new Set();
  const seenAdmissions = { commandHash: new Set(), responseHash: new Set(), keyHash: new Set() };
  let shape;
  for (const { rows, evidence } of captures) {
    const [header] = rows;
    const start = rows.find((row) => row.type === 'start');
    const complete = rows.at(-1);
    check(header?.type === 'run' && start, 'load_window_not_started', true);
    check(complete?.type === 'end' && complete.ms >= end, 'complete_load_window_required');
    check(isHash(header.reservation?.manifestHash) && workloadShape(header) === header.reservation.manifestHash,
      'sample_shape_evidence_invalid', true);
    const { cohortHash, placementHash, runtimeMappingHash, manifestHash } = header.reservation;
    check([cohortHash, placementHash, runtimeMappingHash].every(isHash)
      && header.users.every((user) => isHash(user.subjectHash)), 'qualified_sample_cohort_missing');
    check(evidence?.run === header.run && evidence.startUtc === start.utc && evidence.endUtc === complete.utc
      && evidence.sourceSha === header.candidate.sourceSha && evidence.imageDigest === header.candidate.imageDigest
      && evidence.cohortHash === cohortHash && evidence.placementHash === placementHash
      && evidence.runtimeMappingHash === runtimeMappingHash && isHash(evidence.evidenceHash),
    'evidence_run_candidate_or_window_mismatch');
    check(!runIds.has(header.run), 'capture_reused', true);
    runIds.add(header.run);
    const qualifiedShape = hash(JSON.stringify([manifestHash, cohortHash, placementHash, runtimeMappingHash,
      header.reservation.verifiedK, header.users.map((user) => user.subjectHash)]));
    shape ??= qualifiedShape;
    check(shape === qualifiedShape, 'sample_shape_changed', true);
    const work = workload(rows, header, samples, failed, seenAdmissions);
    const live = streams(rows, samples, failed);
    const oracle = evidence.oracle;
    reconcile(oracle, { missingIsBlocked: false });
    const commandHashes = new Map(work.admitted.map((row) => [row.commandHash, row]));
    failure(oracle.commands.length === commandHashes.size, failed, 'accepted_identity_count_mismatch');
    for (const row of oracle.commands) {
      const client = commandHashes.get(row.commandHash);
      failure(client?.responseHash === row.responseHash && finite(row.nativeToPublicMs)
        && row.nativeToPublicMs <= 30_000, failed, 'oracle_terminal_mismatch');
      if (client && client.at >= steadyStart && client.at < steadyEnd) {
        const user = header.users[client.user];
        groupSamples(samples, `terminal/${client.protocol}/${user.providerClass}/${user.modelClass}`, [row.nativeToPublicMs]);
      }
    }
    check(evidence.streams?.complete === true && isHash(evidence.streams.cursorEvidenceHash)
      && Array.isArray(evidence.streams.expectedEventHashes), 'independent_stream_evidence_missing');
    const expectedEvents = new Set(evidence.streams.expectedEventHashes);
    failure(expectedEvents.size > 0 && expectedEvents.size === evidence.streams.expectedEventHashes.length
      && expectedEvents.size === live.eventHashes.length
      && live.eventHashes.every((value) => expectedEvents.delete(value)), failed, 'stream_replay_or_sequence_gap');
    const fundedCount = Object.entries(work.providerCounts).filter(([name]) => name.includes('/funded/'))
      .reduce((n, [, value]) => n + value.admitted, 0);
    check(isHash(evidence.provider?.quotaEvidenceHash) && isHash(evidence.provider?.creditEvidenceHash)
      && isHash(evidence.provider?.latencyEvidenceHash) && evidence.provider?.healthyQuota === true
      && evidence.provider?.fundedAdmitted === fundedCount, 'funded_provider_evidence_missing');
    check(isHash(evidence.faultEvidenceHash) && Array.isArray(evidence.faults)
      && evidence.faults.every((row) => finite(row.injectedAtMs) && finite(row.recoveredAtMs)
        && row.recoveredAtMs >= row.injectedAtMs && isHash(row.userOutcomeHash)), 'fault_timeline_missing');
    check(isHash(evidence.cleanupEvidenceHash) && evidence.cleanupComplete === true,
      'fixture_cleanup_evidence_missing');
    failure(complete.unresolvedOwners === 0, failed, 'unresolved_owners');
    checkInfrastructure(evidence, samples, failed);
    runs.push({ run: header.run, startUtc: start.utc, endUtc: complete.utc, candidate: header.candidate,
      users: header.users.length, peakStreams: header.users.length, ...work.counts,
      requests: work.requests, providers: work.providerCounts, streamChurn: live.churn,
      evidenceHash: evidence.evidenceHash, faults: evidence.faults.length });
  }
  const strata = {};
  for (const [name, values] of samples) {
    const group = distribution(values);
    strata[name] = group;
    if (group.n < 1000) {
      blocked.push(name);
    } else if (name.startsWith('read/')) {
      failure(group.p95 <= 500 && group.p99 <= 2000, failed, 'management_read_latency_exceeded');
    } else if (name.startsWith('terminal/')) {
      failure(group.p99 <= 5000, failed, 'terminal_latency_exceeded');
    } else if (name.startsWith('mongo/checkout/')) {
      failure(group.p99 < 100, failed, 'infrastructure_threshold_exceeded');
    } else {
      failure(group.p99 <= 2000, failed, 'ack_or_stream_latency_exceeded');
    }
  }
  return { schema: 'cma-lc01-score-v1', status: failed.length ? 'FAIL' : blocked.length ? 'BLOCKED' : 'PASS',
    runs, strata, failures: [...new Set(failed)], insufficientSamples: [...new Set(blocked)] };
}
