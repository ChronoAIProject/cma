import { check, isHash, reconcile } from '../lib/checks.mjs';
import { summarize } from '../lib/metrics.mjs';
import { checkHaReplicaStep, haReplicaPlans, haReplicaTargets } from './scaling.mjs';

export const deterministicCases = [
  'owners_2', 'owners_3', 'partition', 'lease_expiry', 'monotonic_epochs',
  'stale_input', 'stale_result', 'stale_ack', 'stale_terminal', 'cross_ingress_reconnect',
  'relay_auth', 'relay_hop_limit', 'sandbox_desktop_exclusion', 'quota_once', 'admission_global',
  'compatible_readers', 'incompatible_readers', 'switch_off', 'zombie_owner', 'clock_skew',
];

function executionProof(value) {
  check(value && value.passed === true, 'production_contract_failed');
  check(value.productionPath === true && value.mongoReplicaSet === true
    && value.processHashes?.length >= 2 && value.clientHashes?.length >= 2
    && [...value.processHashes, ...value.clientHashes].every(isHash)
    && new Set(value.processHashes).size === value.processHashes.length
    && new Set(value.clientHashes).size === value.clientHashes.length && isHash(value.evidenceHash),
  'independent_production_instances_unproven', true);
}

export async function contractCampaign(driver, { record = () => {} } = {}) {
  check(typeof driver.contractCase === 'function', 'contract_driver_missing', true);
  const evidence = [];
  for (const name of deterministicCases) {
    const proof = await driver.contractCase(name);
    record({ ...proof, name });
    executionProof(proof);
    evidence.push(proof.evidenceHash);
  }
  for (const mode of ['cold_cache', 'warm_cache', 'rollout_surge']) {
    const proof = await driver.contractCase(mode);
    record({ ...proof, name: mode });
    executionProof(proof);
    check(proof.acceptedLost === 0 && proof.duplicateEffects === 0 && proof.staleWrites === 0,
      'rollout_or_cache_correctness_failed');
    evidence.push(proof.evidenceHash);
  }
  return { cases: evidence.length, evidence_hashes: evidence };
}

export async function trialCampaign(driver, role, { record = () => {} } = {}) {
  check(typeof driver.lossTrial === 'function', 'loss_trial_driver_missing', true);
  check(['am', 'eg'].includes(role), 'invalid_trial_role');
  const required = role === 'am' ? 101 : 10;
  const trials = [];
  const trialIds = new Set();
  for (let trial = 0; trial < required; trial++) {
    const mode = role === 'am' ? trial % 2 === 0 ? 'process' : 'graceful'
      : trial < 5 ? 'process' : 'node';
    const proof = await driver.lossTrial({ role, mode, trial, justRenewed: mode !== 'graceful', recoveryMs: 30000 });
    record({ ...proof, role });
    executionProof(proof);
    check(isHash(proof.trialHash) && !trialIds.has(proof.trialHash), 'trial_identity_reused');
    trialIds.add(proof.trialHash);
    check(proof.faultKind === mode && (mode !== 'node' || proof.nodeLossObserved === true), 'node_loss_not_proven', true);
    check(Number.isFinite(proof.recoveryMs) && proof.recoveryMs >= 0 && proof.recoveryMs <= 30000
      && proof.sustainedSuccess === true, 'trial_recovery_failed');
    reconcile(proof.oracle);
    trials.push({ mode, recovery_ms: proof.recoveryMs, evidence_hash: proof.evidenceHash });
  }
  return { role, trials };
}

export async function partitionLoad(driver) {
  check(typeof driver.partitionLoad === 'function', 'partition_driver_missing', true);
  const proofs = [];
  for (const fault of ['worker_panic', 'saturated_shard']) {
    const proof = await driver.partitionLoad({ partitions: 20, hotEventsPerSecond: 10,
      otherEventsPerSecond: 1 / 30, retainedEvents: 100000, activeEvents: 100, replyParts: 8,
      attachments: true, fault });
    executionProof(proof);
    check(proof.partitions === 20 && proof.hotRate >= 10 && proof.otherMinimumRate >= 1 / 30
      && proof.retainedEvents >= 100000 && proof.activeEvents >= 100 && proof.replyParts >= 8
      && proof.attachments === true, 'partition_load_incomplete');
    const stats = summarize(proof.providerStartSamples);
    check(stats.n >= 100 && stats.failed === 0 && stats.p95_ms <= 2000 && stats.max_ms <= 10000
      && proof.preStartExpiry === 0 && proof.unknown === 0 && proof.partial === 0, 'provider_delivery_unproven');
    reconcile(proof.oracle);
    proofs.push({ fault, ...stats, evidence_hash: proof.evidenceHash });
  }
  return { proofs };
}

export async function replicaSweep(driver, { mode = 'manual', record = () => {} } = {}) {
  check(typeof driver.replicaStep === 'function', 'replica_sweep_driver_missing', true);
  check(['manual', 'hpa'].includes(mode), 'ha_scaling_mode_invalid', true);
  const steps = [];
  for (const target of haReplicaTargets) {
    const proof = await driver.replicaStep({ ...target, mode, users: 100,
      streams: 100, turnsPerMinute: 20, readsPerSecond: 10, overlapRollout: false });
    record(proof);
    executionProof(proof);
    checkHaReplicaStep(proof, target, mode);
    steps.push({ target, observed_ready: { am: proof.am.ready, eg: proof.eg.ready },
      evidence_hash: proof.evidenceHash });
  }
  return { mode, plans: haReplicaPlans, steps };
}
