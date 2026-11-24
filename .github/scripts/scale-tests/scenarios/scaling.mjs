import { check, clock, count, isHash, reconcile, waitFor, withinDeadline } from '../lib/checks.mjs';
import { connectedWorkload } from '../lib/load.mjs';
import { scalingSequence } from '../lib/replicas.mjs';

export const haReplicaPlans = Object.freeze({ am: Object.freeze([2, 3, 5, 2]), eg: Object.freeze([2, 3]) });
export const haReplicaTargets = Object.freeze(haReplicaPlans.am.map((am, index) =>
  Object.freeze({ am, eg: haReplicaPlans.eg[Math.min(index, haReplicaPlans.eg.length - 1)] })));

function checkServingSnapshot(snapshot, users) {
  check(snapshot && isHash(snapshot.evidenceHash), 'cluster_snapshot_missing', true);
  check(count(snapshot.connectedUsers) >= users && count(snapshot.openStreams) >= users,
    'connected_user_count_dropped');
  check(count(snapshot.failedRequests) === 0 && count(snapshot.droppedRequests) === 0,
    'user_request_failed_during_scaling');
  check(count(snapshot.streamGaps) === 0 && count(snapshot.staleWrites) === 0,
    'stream_or_fencing_failure');
  for (const role of ['am', 'eg']) {
    check(snapshot[role]?.pdb === true, 'disruption_prerequisite_missing', true);
    count(snapshot[role].ready); count(snapshot[role].desired);
  }
}

export function checkSnapshot(snapshot, users) {
  checkServingSnapshot(snapshot, users);
  for (const role of ['am', 'eg']) {
    const value = snapshot[role];
    check(value && value.hpa === true && value.metricsAvailable === true && value.min === 2
      && value.max === 5 && value.pdb === true, 'hpa_or_disruption_prerequisite_missing', true);
    count(value.ready); count(value.desired);
  }
  return snapshot;
}

export function checkHaReplicaStep(proof, target, mode) {
  check(['manual', 'hpa'].includes(mode) && proof?.scalingMode === mode, 'ha_scaling_mode_invalid', true);
  checkServingSnapshot(proof, 100);
  for (const role of ['am', 'eg']) {
    const value = proof[role];
    check(value.ready === target[role] && value.desired === target[role], 'replica_sweep_failed');
    check(Array.isArray(value.servingReplicaHashes) && value.servingReplicaHashes.length === value.ready
      && value.servingReplicaHashes.every(isHash)
      && new Set(value.servingReplicaHashes).size === value.ready, 'serving_replicas_unproven', true);
    if (mode === 'hpa') {
      check(value.hpa === true && value.metricsAvailable === true && value.min === 2
        && Number.isSafeInteger(value.max) && value.max >= Math.max(...haReplicaPlans[role])
        && value.hpaEventsObserved === true && ['cpu', 'connections'].includes(value.trigger),
      'hpa_or_disruption_prerequisite_missing', true);
    }
  }
  check(count(proof.minimumConnectedUsers) >= 100 && count(proof.minimumOpenStreams) >= 100
    && Number.isFinite(proof.actualReadsPerSecond) && proof.actualReadsPerSecond >= 10
    && Number.isFinite(proof.actualTurnsPerMinute) && proof.actualTurnsPerMinute >= 20
    && count(proof.attemptedRequests) > 0, 'ha_workload_not_maintained');
  check(count(proof.wrongRoutes) === 0, 'wrong_route_during_scaling');
  check(Number.isFinite(proof.recoveryMs) && proof.recoveryMs >= 0 && proof.recoveryMs <= 30000
    && proof.sustainedSuccess === true, 'scaling_recovery_failed');
  check(Number.isFinite(proof.mongoHeadroom) && proof.mongoHeadroom >= 0.30 && proof.mongoHeadroom <= 1
    && proof.allClientsAccounted === true && proof.terminatingProcessesWithinBound === true,
  'capacity_evidence_missing', true);
  reconcile(proof.oracle);
  return proof;
}

export async function autoscale(driver, config, { users = 2, time = clock } = {}) {
  check(users === 2 || users === 100, 'invalid_user_cohort');
  const peak = config.scalingPeak ?? 4;
  const sequence = scalingSequence(peak);
  let cohort; let stress;
  const observations = [];
  const sample = async (remaining, signal) => {
    signal.throwIfAborted();
    const snapshot = await driver.snapshot(cohort, { timeoutMs: remaining, signal });
    signal.throwIfAborted();
    const value = checkSnapshot(snapshot, users);
    observations.push(value);
    check(['am', 'eg'].every((role) => value[role].ready <= peak && value[role].desired <= peak),
      'scaling_peak_exceeded');
    return value;
  };
  const both = (row, accept) => ['am', 'eg'].every((role) => accept(row[role]));
  const observe = (run) => withinDeadline(run, config.scaleTimeoutMs, 'scaling_observation_deadline_exceeded');
  const observeReplicas = (count) => observe((signal) =>
    waitFor((remaining) => sample(remaining, signal),
      (row) => both(row, (role) => role.ready === count && role.desired === count),
      { timeoutMs: config.scaleTimeoutMs, intervalMs: 1000, time }));
  try {
    cohort = await driver.connectUsers({ ...connectedWorkload, users, streams: users });
    const initial = await observe((signal) => sample(config.scaleTimeoutMs, signal));
    check(both(initial, (role) => role.ready === 2 && role.desired === 2), 'initial_replicas_not_two', true);
    stress = await driver.startStress(cohort, { targetReplicas: peak });
    await observeReplicas(peak);
    await driver.stopStress(stress); stress = null;
    // Connected users and canaries continue through the downscale stabilization window.
    await observeReplicas(2);
    const final = await driver.finishUsers(cohort);
    checkSnapshot(final, users);
    check(count(final.attemptedRequests) > 0 && final.hpaEventsObserved === true,
      'load_or_hpa_events_missing', true);
    if (users === 100) {
      check(final.actualReadsPerSecond >= 10 && final.actualTurnsPerMinute >= 20
        && final.minimumConnectedUsers >= 100 && final.minimumOpenStreams >= 100,
      'lc16_workload_not_maintained');
    }
    reconcile(await driver.reconcile(cohort));
    return { users, sequence, observations: observations.length, requests: final.attemptedRequests,
      failed: final.failedRequests, peak_am: Math.max(...observations.map((o) => o.am.ready)),
      peak_eg: Math.max(...observations.map((o) => o.eg.ready)), final_am: final.am.ready, final_eg: final.eg.ready };
  } finally {
    try { if (stress) await driver.stopStress(stress); }
    finally { if (cohort) await driver.cleanup(cohort); }
  }
}
