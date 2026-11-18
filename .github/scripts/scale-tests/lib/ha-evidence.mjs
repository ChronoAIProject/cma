import { check, count, hash, isHash, reconcile, safeFailure } from './checks.mjs';
import { deterministicCases } from '../scenarios/contract.mjs';
import { checkHaReplicaStep, haReplicaTargets } from '../scenarios/scaling.mjs';

const candidateFields = ['sourceSha', 'amImageDigest', 'egImageDigest', 'runtimeImageDigest',
  'configHash', 'protocolHash', 'environmentHash'];
const contractCases = [...deterministicCases, 'cold_cache', 'warm_cache', 'rollout_surge'];

// Environment and raw-result references are opaque hashes; mappings stay in the private packet.
export function haCandidateHash(candidate) {
  check(candidate && /^[a-f0-9]{40}$/.test(candidate.sourceSha), 'ha_source_sha_missing', true);
  for (const field of ['amImageDigest', 'egImageDigest', 'runtimeImageDigest']) {
    check(typeof candidate[field] === 'string' && /^sha256:[a-f0-9]{64}$/.test(candidate[field]),
      'ha_image_digest_missing', true);
  }
  for (const field of ['configHash', 'protocolHash', 'environmentHash']) {
    check(isHash(candidate[field]), 'ha_candidate_identity_missing', true);
  }
  return hash(JSON.stringify(candidateFields.map((field) => candidate[field])));
}

function hashes(values, minimum, code) {
  check(Array.isArray(values) && values.length >= minimum && values.every(isHash)
    && new Set(values).size === values.length, code, true);
}

function window(value) {
  check(Number.isSafeInteger(value?.startedAtMs) && value.startedAtMs >= 0
    && Number.isSafeInteger(value.finishedAtMs) && value.finishedAtMs > value.startedAtMs,
  'ha_observation_window_invalid', true);
}

function independent(value) {
  check(value.passed === true && value.productionPath === true && value.mongoReplicaSet === true,
    'ha_production_path_unproven', true);
  hashes(value.processHashes, 2, 'ha_independent_processes_missing');
  hashes(value.clientHashes, 2, 'ha_independent_clients_missing');
}

function correctness(value) {
  for (const field of ['wrongRoutes', 'staleWrites', 'duplicateEffects', 'acceptedLost', 'streamGaps']) {
    check(count(value[field]) === 0, 'ha_correctness_failure');
  }
  reconcile(value.oracle);
}

function workload(value) {
  check(Number.isSafeInteger(value.minimumConnectedUsers) && value.minimumConnectedUsers >= 100
    && Number.isSafeInteger(value.minimumOpenStreams) && value.minimumOpenStreams >= 100
    && Number.isFinite(value.actualReadsPerSecond) && value.actualReadsPerSecond >= 10
    && Number.isFinite(value.actualTurnsPerMinute) && value.actualTurnsPerMinute >= 20,
  'ha_workload_missing', true);
}

function recovery(value, limit) {
  check(Number.isSafeInteger(value.lossStartedAtMs) && Number.isSafeInteger(value.sustainedSuccessAtMs)
    && value.lossStartedAtMs >= value.startedAtMs && value.sustainedSuccessAtMs >= value.lossStartedAtMs
    && value.sustainedSuccessAtMs <= value.finishedAtMs && value.sustainedSuccess === true,
  'ha_fault_timing_missing', true);
  const elapsed = value.sustainedSuccessAtMs - value.lossStartedAtMs;
  check(value.recoveryMs === elapsed && elapsed <= limit, 'ha_recovery_failed');
}

function fault(value) {
  recovery(value, 30000);
  check(value.faultStartedAtMs === value.lossStartedAtMs
    && Number.isSafeInteger(value.faultObservedAtMs) && value.faultObservedAtMs >= value.lossStartedAtMs
    && value.faultObservedAtMs <= value.sustainedSuccessAtMs, 'ha_fault_timing_missing', true);
  check(value.processDeathObserved === true && isHash(value.lostProcessHash)
    && value.processHashes.includes(value.lostProcessHash) && isHash(value.lostIncarnationHash)
    && isHash(value.faultEvidenceHash), 'ha_process_death_unproven', true);
  if (['node', 'node_drain'].includes(value.faultKind)) {
    check(value.nodeLossObserved === true && isHash(value.lostNodeHash), 'ha_node_loss_unproven', true);
  }
  if (['graceful', 'node_drain'].includes(value.faultKind)) {
    check(value.drainStartedAtMs === value.lossStartedAtMs, 'ha_drain_time_excluded');
  } else {
    check(value.leaseJustRenewed === true, 'ha_worst_case_lease_missing', true);
  }
  hashes(value.affectedAcceptedHashes, 1, 'ha_affected_work_missing');
  check(value.affectedAcceptedHashes.every((id) => value.oracle.acceptedHashes.includes(id)),
    'ha_affected_work_not_reconciled');
  workload(value);
}

export function validateHaEvidence(expectedCandidate, observations) {
  const failures = [];
  const counts = { am: 0, eg: 0, scaling: 0, contracts: 0, disruptions: 0, repairs: 0 };
  const verify = (section, index, action) => {
    try { action(); return true; }
    catch (error) { failures.push({ section, index, ...safeFailure(error) }); return false; }
  };
  const result = () => ({ status: failures.some((f) => f.status === 'FAIL') ? 'FAIL'
    : failures.length ? 'BLOCKED' : 'PASS', failures, counts });
  let candidateHash;
  if (!verify('candidate', null, () => { candidateHash = haCandidateHash(expectedCandidate); })) return result();
  const common = (value) => {
    check(value && value.candidateHash === candidateHash, 'ha_candidate_mismatch', true);
    check(value.runHash === observations.runHash && isHash(value.evidenceHash), 'ha_evidence_identity_missing', true);
    window(value);
    check(value.startedAtMs >= observations.startedAtMs && value.finishedAtMs <= observations.finishedAtMs,
      'ha_observation_outside_run', true);
    check(value.completed === true && value.cancelled === false, 'ha_run_incomplete', true);
    check(value.attempt === 1 && value.firstRunOutcome === 'PASS', 'ha_first_run_failed');
  };
  if (!verify('run', null, () => {
    check(observations?.schema === 'cma-ha-evidence-v1', 'ha_schema_invalid', true);
    check(haCandidateHash(observations.candidate) === candidateHash, 'ha_candidate_mismatch', true);
    check(isHash(observations.runHash) && isHash(observations.rawEvidenceHash), 'ha_run_identity_missing', true);
    common(observations);
  })) return result();

  const rows = (section) => {
    const values = observations[section];
    verify(section, null, () => check(Array.isArray(values) && values.length > 0, 'ha_samples_missing', true));
    return Array.isArray(values) ? values : [];
  };
  const identities = new Set();
  const identify = (value) => {
    check(isHash(value.trialHash) && !identities.has(value.trialHash), 'ha_trial_identity_reused');
    identities.add(value.trialHash);
  };
  const modes = { am: { process: 0, graceful: 0 }, eg: { process: 0, node: 0 } };
  for (const [index, value] of rows('trials').entries()) {
    verify('trials', index, () => {
      common(value); identify(value); independent(value); correctness(value);
      check(['am', 'eg'].includes(value.role) && Object.hasOwn(modes[value.role], value.faultKind),
        'ha_trial_kind_invalid');
      fault(value);
      counts[value.role]++; modes[value.role][value.faultKind]++;
    });
  }
  verify('trials', null, () => {
    check(counts.am >= 101 && counts.eg >= 10 && modes.am.process >= 51 && modes.am.graceful >= 50
      && modes.eg.process >= 5 && modes.eg.node >= 5, 'ha_trial_count_incomplete', true);
    check(observations.declaredCounts?.am === counts.am && observations.declaredCounts?.eg === counts.eg,
      'ha_sample_count_mismatch');
  });

  const scaling = rows('scaling');
  verify('scaling', null, () => check(scaling.length === haReplicaTargets.length, 'ha_scale_plan_incomplete', true));
  for (const [index, value] of scaling.entries()) {
    verify('scaling', index, () => {
      common(value); independent(value); correctness(value); workload(value); recovery(value, 30000);
      check(index < haReplicaTargets.length && (!index || value.startedAtMs >= scaling[index - 1].finishedAtMs),
        'ha_scale_order_invalid');
      checkHaReplicaStep(value, haReplicaTargets[index], observations.scalingMode);
      counts.scaling++;
    });
  }

  const names = new Set();
  for (const [index, value] of rows('contracts').entries()) {
    verify('contracts', index, () => {
      common(value); independent(value); correctness(value);
      check(contractCases.includes(value.name) && !names.has(value.name), 'ha_contract_case_invalid');
      names.add(value.name); counts.contracts++;
    });
  }
  verify('contracts', null, () => check(names.size === contractCases.length, 'ha_contract_cases_missing', true));

  const disruptions = new Set();
  for (const [index, value] of rows('disruptions').entries()) {
    verify('disruptions', index, () => {
      common(value); identify(value); independent(value); correctness(value);
      check(['am', 'eg'].includes(value.role) && ['node', 'node_drain'].includes(value.faultKind),
        'ha_disruption_kind_invalid');
      fault(value);
      check(isHash(value.homeHashBefore) && value.homeHashBefore === value.homeHashAfter
        && value.manualDatabaseRepair === false && value.pendingOverFiveMinutes === 0
        && value.nPlusOneCapacityMaintained === true, 'ha_disruption_recovery_unproven', true);
      disruptions.add(`${value.role}:${value.faultKind}`); counts.disruptions++;
    });
  }
  verify('disruptions', null, () => check(disruptions.size === 4, 'ha_node_disruptions_missing', true));

  for (const [index, value] of rows('repairs').entries()) {
    verify('repairs', index, () => {
      common(value); identify(value); independent(value); correctness(value); recovery(value, 1800000);
      check(value.forwardRepair === true && value.historyAvailable === true && value.newTurnAvailable === true
        && value.databaseRollback === false && value.homeDeleted === false && value.asset404 === 0
        && value.reconnectContractProven === true, 'ha_compatible_repair_unproven', true);
      counts.repairs++;
    });
  }
  verify('repairs', null, () => check(counts.repairs >= 2, 'ha_compatible_repairs_missing', true));

  const prerequisites = observations.prerequisites;
  for (const role of ['am', 'eg']) verify(`deployment_${role}`, null, () => {
    const value = prerequisites?.[role]; common(value);
    check(value.pdbApiVersion === 'policy/v1' && value.effectiveSurvivalFloor >= 1
      && Number.isSafeInteger(value.effectiveSurvivalFloor) && value.pdbEffective === true
      && value.independentlyServing === true && value.separateNodes === true
      && value.readinessWithdrawal === true && value.signedPreStop === true && value.boundedTermination === true
      && value.surgeBounded === true && value.terminatingBounded === true && value.sparePlacementCapacity === true,
    'ha_deployment_prerequisites_missing', true);
  });
  verify('atlas', null, () => {
    const value = prerequisites?.atlas; common(value);
    check(['cpuHeadroom', 'connectionHeadroom', 'storageHeadroom'].every((field) =>
      Number.isFinite(value[field]) && value[field] >= 0.30 && value[field] <= 1)
      && value.allClientsAccounted === true && value.surgeAndTerminatingIncluded === true
      && value.otherConsumersIncluded === true && isHash(value.poolBudgetHash), 'ha_atlas_evidence_missing', true);
  });
  verify('workers', null, () => {
    const value = prerequisites?.workers; common(value);
    check(value.passed === true && isHash(value.inventoryHash), 'ha_worker_inventory_missing', true);
  });
  verify('compatibility', null, () => {
    const value = prerequisites?.compatibility; common(value);
    check(value.coordinatedProtocol === true && value.retainedRuntimeCompatible === true
      && value.legacyTransitionNoOverlap === true, 'ha_protocol_compatibility_missing', true);
  });
  return result();
}
