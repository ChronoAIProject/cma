import { check, clock, hash, safeFailure } from './checks.mjs';
import { measurePerformance } from './measurement.mjs';
import { scenarioNames } from './report.mjs';
import { validateConfig } from './safety.mjs';
import { crossReplica, suddenLoss } from '../scenarios/correctness.mjs';
import { autoscale } from '../scenarios/scaling.mjs';
import { contractCampaign, partitionLoad, replicaSweep, trialCampaign } from '../scenarios/contract.mjs';
import { catalogSha256 } from './catalog.mjs';

export async function runSuite(config, driver, { time = clock, progress = () => {}, checkpoint, samples, scenario = 'all' } = {}) {
  const report = { schema: 'cma-scale-results-v1', environment: 'local', started_at: new Date().toISOString(),
    status: 'BLOCKED', track_status: 'INCOMPLETE', catalog_sha256: catalogSha256, scenarios: [], performance: [] };
  const allBlocked = (code) => {
    report.scenarios = (scenario === 'baseline-n1' ? ['n1_capture'] : scenarioNames)
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
    check(['all', 'baseline-n1'].includes(scenario), 'invalid_scenario', true);
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
    const names = scenario === 'baseline-n1' ? ['n1_capture'] : scenarioNames;
    const selected = scenario === 'baseline-n1'
      ? [() => measurePerformance(driver, config, report, { checkpoint, samples, baselineOnly: true })] : actions;
    for (let i = 0; i < selected.length; i++) {
      const row = await run(names[i], selected[i]);
      if (row.status !== 'PASS') {
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
