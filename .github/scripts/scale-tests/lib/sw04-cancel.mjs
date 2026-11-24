import { check, safeFailure } from './checks.mjs';
import { sw04Scenario, sw04Selection } from './report.mjs';
import { sw04Cancel } from '../scenarios/correctness.mjs';

export function verifySw04Fixture(log) {
  const frames = [...log.matchAll(/^(?:test \S+ \.\.\. )?SW04_CANCEL_OBSERVATION=(.*)$/gm)];
  check(frames.length > 0, 'sw04_observation_missing', true);
  check(frames.length === 1, 'sw04_observation_duplicate');
  let observation;
  try {
    observation = JSON.parse(frames[0][1]);
  } catch {
    check(false, 'sw04_observation_malformed');
  }
  return sw04Cancel(observation);
}

export function runSw04Fixture(log) {
  const report = {
    schema: 'cma-scale-results-v1', environment: 'local', selection: sw04Selection,
    started_at: new Date().toISOString(), track_status: 'INCOMPLETE',
  };
  try {
    const evidence = verifySw04Fixture(log);
    return { ...report, status: 'NO_GO',
      scenarios: [{ scenario: sw04Scenario, status: 'PASS', evidence_hash: evidence.evidence_hash, evidence }] };
  } catch (error) {
    const failure = safeFailure(error);
    return { ...report, status: failure.status, scenarios: [{ scenario: sw04Scenario, ...failure }] };
  }
}
