import { check, clock, count, isHash, reconcile, waitFor } from '../lib/checks.mjs';
import { connectedWorkload } from '../lib/load.mjs';
import { scalingSequence } from '../lib/replicas.mjs';

export function checkSnapshot(snapshot, users) {
  check(snapshot && isHash(snapshot.evidenceHash), 'cluster_snapshot_missing', true);
  check(count(snapshot.connectedUsers) >= users && count(snapshot.openStreams) >= users,
    'connected_user_count_dropped');
  check(count(snapshot.failedRequests) === 0 && count(snapshot.droppedRequests) === 0,
    'user_request_failed_during_scaling');
  check(count(snapshot.streamGaps) === 0 && count(snapshot.staleWrites) === 0,
    'stream_or_fencing_failure');
  for (const role of ['am', 'eg']) {
    const value = snapshot[role];
    check(value && value.hpa === true && value.metricsAvailable === true && value.min === 2
      && value.max === 5 && value.pdb === true, 'hpa_or_disruption_prerequisite_missing', true);
    count(value.ready); count(value.desired);
  }
  return snapshot;
}

export async function autoscale(driver, config, { users = 2, time = clock } = {}) {
  check(users === 2 || users === 100, 'invalid_user_cohort');
  const peak = config.scalingPeak ?? 4;
  const sequence = scalingSequence(peak);
  let cohort; let stress;
  const observations = [];
  const sample = async (remaining) => {
    const value = checkSnapshot(await driver.snapshot(cohort, { timeoutMs: remaining }), users);
    observations.push(value);
    return value;
  };
  const both = (row, accept) => ['am', 'eg'].every((role) => accept(row[role]));
  try {
    cohort = await driver.connectUsers({ ...connectedWorkload, users, streams: users });
    const initial = await sample(config.scaleTimeoutMs);
    check(both(initial, (role) => role.ready === 2 && role.desired === 2), 'initial_replicas_not_two', true);
    stress = await driver.startStress(cohort, { targetReplicas: peak });
    await waitFor(sample, (row) => both(row, (role) => role.ready === peak && role.desired === peak),
      { timeoutMs: config.scaleTimeoutMs, intervalMs: 1000, time });
    await driver.stopStress(stress); stress = null;
    // Connected users and canaries continue through the downscale stabilization window.
    await waitFor(sample, (row) => both(row, (role) => role.ready === 2 && role.desired === 2),
      { timeoutMs: config.scaleTimeoutMs, intervalMs: 1000, time });
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
