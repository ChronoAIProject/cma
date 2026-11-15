import { check, clock, isHash, reconcile, waitFor } from '../lib/checks.mjs';

export const paths = ['create', 'open', 'send', 'interrupt', 'stream', 'desktop'];

function pathProof(value, replicaHash) {
  check(value?.ok === true && value.completed === true, 'client_path_failed');
  check(isHash(value.evidenceHash) && isHash(value.servingReplicaHash), 'path_provenance_missing', true);
  if (replicaHash) check(value.servingReplicaHash === replicaHash, 'wrong_replica_served');
}

export async function crossReplica(driver) {
  const routes = await driver.routes();
  check(Array.isArray(routes) && routes.length >= 3, 'replica_routes_missing', true);
  const direct = routes.filter((r) => r.kind === 'replica');
  check(direct.length >= 2 && direct.every((r) => isHash(r.replicaHash))
    && new Set(direct.map((r) => r.replicaHash)).size === direct.length,
  'independent_replicas_not_proven', true);
  check(routes.some((r) => r.kind === 'balancer'), 'balancer_route_missing', true);
  const coverage = new Map(paths.map((path) => [path, new Set()]));
  let journeys = 0;
  for (const source of routes) {
    for (const target of routes) {
      let agent; let stream;
      try {
        agent = await driver.create(source, { desktop: true });
        pathProof(agent.proof, source.replicaHash);
        coverage.get('create').add(agent.proof.servingReplicaHash);
        stream = await driver.connect(agent, target);
        for (const path of ['open', 'send', 'interrupt', 'desktop']) {
          const proof = await driver.action(path, agent, target);
          pathProof(proof, target.replicaHash);
          if (path === 'desktop') check(proof.frameReceived && proof.exclusive, 'desktop_not_proven');
          if (path === 'open') check(proof.rendered === true, 'agent_open_not_rendered');
          coverage.get(path).add(proof.servingReplicaHash);
        }
        const proof = await driver.confirmStream(stream);
        pathProof(proof, target.replicaHash);
        check(proof.liveEvent === true && proof.durableContinuity === true, 'stream_delivery_not_proven');
        coverage.get('stream').add(proof.servingReplicaHash);
        reconcile(await driver.reconcile(agent));
        journeys++;
      } finally {
        // Cleanup is part of the gate; failures propagate rather than silently leaking fixtures.
        try { if (stream) await driver.disconnect(stream); }
        finally { if (agent) await driver.cleanup(agent); }
      }
    }
  }
  for (const path of paths) {
    check(direct.every((route) => coverage.get(path).has(route.replicaHash)), 'path_replica_coverage_incomplete');
  }
  return { journeys, paths: paths.length, replicas: direct.length };
}

export async function suddenLoss(driver, mode, config, time = clock) {
  check(['am_stream', 'am_command', 'eg_inbound'].includes(mode), 'invalid_loss_mode');
  let work;
  try {
    work = await driver.beginInFlight(mode);
    check(work?.accepted === true && work.inFlight === true && isHash(work.commandHash)
      && isHash(work.ownerHash) && isHash(work.evidenceHash), 'inflight_boundary_not_proven', true);
    if (mode !== 'eg_inbound') check(work.leaseJustRenewed === true, 'worst_case_lease_not_proven', true);
    const lossStarted = time.now();
    const loss = await driver.killOwner(work);
    check(loss?.ownerHash === work.ownerHash && loss.abrupt === true
      && loss.processDeathObserved === true && isHash(loss.evidenceHash), 'abrupt_process_death_unproven', true);
    const observations = [];
    await waitFor(async (remaining) => {
      const result = await driver.observeRecovery(work, { timeoutMs: remaining });
      observations.push(result);
      return result;
    }, (row) => row?.clientRecovered === true && row.sustainedSuccess === true, {
      timeoutMs: Math.max(1, config.recoveryMs - (time.now() - lossStarted)), time,
    });
    const recoveredMs = time.now() - lossStarted;
    check(recoveredMs <= config.recoveryMs, 'recovery_deadline_exceeded');
    check(observations.every((row) => row.accounted === true), 'fault_window_unaccounted', true);
    const oracle = await driver.reconcile(work);
    const proof = reconcile(oracle);
    check(oracle.acceptedHashes.includes(work.commandHash), 'original_fault_command_missing');
    check(proof.commands >= 1, 'fault_work_missing');
    if (mode === 'am_stream') {
      const stream = await driver.confirmStream(work);
      check(stream.durableContinuity === true && stream.liveEvent === true, 'stream_replay_gap');
    }
    return { mode, recovery_ms: recoveredMs, observations: observations.length, ...proof };
  } finally {
    if (work) await driver.cleanup(work);
  }
}
