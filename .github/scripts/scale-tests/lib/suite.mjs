import { isDeepStrictEqual } from 'node:util';
import { check, clock, hash, isHash, safeFailure } from './checks.mjs';
import { measurePerformance } from './measurement.mjs';
import { selectedScenarioNames, selections, recordHaObservation } from './report.mjs';
import { haCandidateHash, validateHaEvidence } from './ha-evidence.mjs';
import { validateConfig } from './safety.mjs';
import { crossReplica, suddenLoss } from '../scenarios/correctness.mjs';
import { autoscale } from '../scenarios/scaling.mjs';
import { contractCampaign, partitionLoad, replicaSweep, trialCampaign } from '../scenarios/contract.mjs';
import { catalogSha256 } from './catalog.mjs';

export async function runSuite(config, driver, { time = clock, progress = () => {}, checkpoint, samples, scenario = 'all' } = {}) {
  const report = { schema: 'cma-scale-results-v1', environment: 'local', started_at: new Date().toISOString(),
    status: 'BLOCKED', track_status: 'INCOMPLETE', catalog_sha256: catalogSha256, scenarios: [], performance: [] };
  const allBlocked = (code) => {
    report.scenarios = selectedScenarioNames(scenario)
      .map((scenario) => ({ scenario, status: 'BLOCKED', code }));
    return report;
  };
  const close = async () => {
    report.cleanup = { attempted: Boolean(driver), completed: !driver };
    if (!driver) return;
    try {
      await driver.close();
      report.cleanup.completed = true;
    } catch (error) { report.scenarios.push({ scenario: 'cleanup', ...safeFailure(error) }); }
  };
  try {
    check(selections.includes(scenario), 'invalid_scenario', true);
    report.selection = scenario;
    validateConfig(config);
    report.provenance = Object.fromEntries(['hardwareHash', 'fixtureHash', 'workloadHash', 'sourceHash', 'imagesHash']
      .map((key) => [key, config[key]]));
    check(driver, 'environment_driver_missing', true);
    const proof = await driver.preflight(config);
    check(proof?.local === true && proof.isolated === true && proof.reviewed === true
      && proof.syntheticUsers === true && proof.mutationWindowGranted === true,
    'environment_preflight_incomplete', true);
    for (const key of ['hardwareHash', 'fixtureHash', 'workloadHash', 'sourceHash', 'imagesHash']) {
      check(proof[key] === config[key], 'environment_provenance_changed', true);
    }
    if (scenario === 'ha') {
      check(proof.haCandidateHash === haCandidateHash(config.haCandidate), 'ha_preflight_candidate_mismatch', true);
      check(['manual', 'hpa'].includes(config.haScalingMode), 'ha_scaling_mode_invalid', true);
      check(typeof driver.haEvidence === 'function', 'ha_evidence_driver_missing', true);
    }
  } catch (error) {
    const failure = safeFailure(error);
    allBlocked(failure.code);
    await close();
    return report;
  }
  const run = async (scenario, action) => {
    const started = time.now();
    let row;
    try {
      const evidence = await action();
      row = { scenario, status: 'PASS', elapsed_ms: time.now() - started,
        evidence_hash: hash(JSON.stringify(evidence)), evidence };
    } catch (error) { row = { scenario, ...safeFailure(error), elapsed_ms: time.now() - started }; }
    report.scenarios.push(row);
    progress(`${row.status === 'PASS' ? 'DONE' : 'BLOCKED'}: ${scenario} ${row.status} ${row.code ?? row.evidence_hash}`);
    return row;
  };
  const observations = { contracts: [], trials: [], scaling: [] };
  if (scenario === 'ha') report.ha = { status: 'BLOCKED', failures: [], records: [] };
  const record = (section) => (value) => {
    observations[section].push(structuredClone(value));
    report.ha.records.push(recordHaObservation(section, value));
  };
  const qualifyHa = async () => {
    const metadata = await driver.haEvidence();
    for (const section of Object.keys(observations)) {
      check(metadata?.[section] === undefined || isDeepStrictEqual(metadata[section], observations[section]),
        'ha_recorded_observations_changed');
    }
    const packet = { ...metadata, ...observations };
    const validation = validateHaEvidence(config.haCandidate, packet);
    report.ha = { ...report.ha, ...validation, candidate_hash: haCandidateHash(config.haCandidate),
      raw_evidence_hash: isHash(packet.rawEvidenceHash) ? packet.rawEvidenceHash : null };
    for (const section of ['disruptions', 'repairs']) {
      for (const value of Array.isArray(packet[section]) ? packet[section] : []) {
        report.ha.records.push(recordHaObservation(section, value));
      }
    }
    check(validation.status === 'PASS', 'ha_evidence_not_passed', validation.status === 'BLOCKED');
    return validation;
  };
  try {
    const actions = [
      () => crossReplica(driver),
      () => suddenLoss(driver, 'am_stream', config, time),
      () => suddenLoss(driver, 'am_command', config, time),
      () => suddenLoss(driver, 'eg_inbound', config, time),
      () => autoscale(driver, config, { time }),
      () => autoscale(driver, config, { users: 100, time }),
      () => contractCampaign(driver),
      () => replicaSweep(driver),
      () => trialCampaign(driver, 'am'),
      () => trialCampaign(driver, 'eg'),
      () => partitionLoad(driver),
      () => measurePerformance(driver, config, report, { checkpoint, samples }),
    ];
    const names = selectedScenarioNames(scenario);
    const selected = scenario === 'baseline-n1'
      ? [() => measurePerformance(driver, config, report, { checkpoint, samples, baselineOnly: true })]
      : scenario === 'ha' ? [
        () => contractCampaign(driver, { record: record('contracts') }),
        () => replicaSweep(driver, { mode: config.haScalingMode, record: record('scaling') }),
        () => trialCampaign(driver, 'am', { record: record('trials') }),
        () => trialCampaign(driver, 'eg', { record: record('trials') }), qualifyHa,
      ] : actions;
    for (let i = 0; i < selected.length; i++) {
      const row = await run(names[i], selected[i]);
      if (row.status !== 'PASS') {
        if (report.ha) {
          report.ha.status = row.status;
          report.ha.failures.push({ section: names[i], index: null, status: row.status, code: row.code });
        }
        // A correctness failure invalidates later load/performance evidence and may leave cleanup unresolved.
        report.scenarios.push(...names.slice(i + 1).map((scenario) => ({ scenario,
          status: 'BLOCKED', code: 'prior_gate_not_passed' })));
        break;
      }
    }
  } finally {
    await close();
  }
  // External baseline/rollout/provider qualification is not implemented by this local subset.
  // Preserve its absence as a machine-visible blocker, not just a runbook caveat.
  report.scenarios.push({ scenario: 'baseline_rollout_provider_qualification', status: 'BLOCKED',
    code: 'external_qualification_integration_pending' });
  report.status = 'NO_GO';
  return report;
}
