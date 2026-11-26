import { check, clock, hash, isHash, reconcile, waitFor, withinDeadline } from '../lib/checks.mjs';

export const paths = ['create', 'open', 'send', 'interrupt', 'stream', 'desktop'];

export function sw04Cancel(observation) {
  check(observation?.schema === 'cma-sw04-cancel-v1', 'sw04_observation_missing', true);
  const { replicas, owner, admission, replay, durable, effectsBeforeReplay, effectsAfterReplay } = observation;
  check(Array.isArray(replicas) && replicas.length === 2 && replicas.every(isHash)
    && new Set(replicas).size === 2, 'sw04_replica_identity_missing', true);
  check(replicas.includes(owner?.processHash) && Number.isSafeInteger(owner.epoch) && owner.epoch > 0,
    'sw04_owner_authority_missing', true);
  check(admission?.status === 202 && replicas.includes(admission.processHash)
    && admission.processHash !== owner.processHash, 'sw04_wrong_ingress');
  check(replay?.status === 202 && replay.processHash === owner.processHash, 'sw04_wrong_replay_ingress');
  const { commandHash, responseHash } = admission;
  check(isHash(commandHash) && isHash(responseHash) && replay.commandHash === commandHash
    && replay.responseHash === responseHash, 'sw04_replay_identity_changed');
  check(durable?.commandHash === commandHash && durable.responseHash === responseHash
    && durable.status === 'completed' && durable.holderHash === owner.processHash
    && Number.isSafeInteger(durable.runtimeGeneration) && durable.runtimeGeneration > 0,
    'sw04_durable_outcome_invalid');
  for (const effects of [effectsBeforeReplay, effectsAfterReplay]) {
    check(Array.isArray(effects), 'sw04_receiver_observation_missing', true);
    check(effects.length > 0, 'sw04_effect_missing');
    check(effects.length === 1, 'sw04_effect_duplicate');
    const [effect] = effects;
    check(effect?.processHash === owner.processHash && effect.ownerProcessHash === owner.processHash
      && effect.epoch === owner.epoch, 'sw04_effect_owner_invalid');
    check(effect.kind === 'interrupt' && effect.commandHash === commandHash
      && effect.runtimeGeneration === durable.runtimeGeneration, 'sw04_effect_identity_invalid');
  }
  return {
    commands: 1, effects: 1, additional_replay_effects: 0,
    evidence_hash: hash(JSON.stringify([admission.processHash, owner.processHash, owner.epoch,
      commandHash, responseHash, durable.runtimeGeneration])),
  };
}

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
    const { recoveredMs, observations } = await observeLoss(driver, work, config.recoveryMs, time);
    const oracle = await driver.reconcile(work);
    const proof = reconcile(oracle);
    check(oracle.acceptedHashes.includes(work.commandHash), 'original_fault_command_missing');
    check(proof.commands >= 1, 'fault_work_missing');
    if (mode === 'am_stream') {
      const stream = await driver.confirmStream(work);
      check(stream.durableContinuity === true && stream.liveEvent === true, 'stream_replay_gap');
    }
    return { mode, recovery_ms: recoveredMs, observations, ...proof };
  } finally {
    if (work) await driver.cleanup(work);
  }
}

async function observeLoss(driver, work, recoveryMs, time) {
  const lossStarted = time.now();
  return withinDeadline(async (signal) => {
    const loss = await driver.killOwner(work, { timeoutMs: recoveryMs, signal });
    signal.throwIfAborted();
    check(loss?.ownerHash === work.ownerHash && loss.abrupt === true
      && loss.processDeathObserved === true && isHash(loss.evidenceHash), 'abrupt_process_death_unproven', true);
    const remaining = recoveryMs - (time.now() - lossStarted);
    check(remaining > 0, 'recovery_deadline_exceeded');
    const observations = [];
    await waitFor(async (timeoutMs) => {
      signal.throwIfAborted();
      const result = await driver.observeRecovery(work, { timeoutMs, signal });
      signal.throwIfAborted();
      observations.push(result);
      return result;
    }, (row) => row?.clientRecovered === true && row.sustainedSuccess === true, { timeoutMs: remaining, time });
    const recoveredMs = time.now() - lossStarted;
    check(recoveredMs <= recoveryMs, 'recovery_deadline_exceeded');
    check(observations.every((row) => row?.accounted === true), 'fault_window_unaccounted', true);
    return { recoveredMs, observations: observations.length };
  }, recoveryMs, 'recovery_deadline_exceeded');
}
