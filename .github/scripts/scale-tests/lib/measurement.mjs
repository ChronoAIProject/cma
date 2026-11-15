import { check } from './checks.mjs';
import { calibrateNoise, compareCohorts, summarize } from './metrics.mjs';
import { criteria, requiredRoutes, userLimits } from './catalog.mjs';
import { expectedReplicas } from './replicas.mjs';

export async function measurePerformance(driver, config, report, { checkpoint, samples, baselineOnly = false }) {
  check(typeof checkpoint === 'function', 'noise_preregistration_required', true);
  check(typeof samples === 'function', 'raw_samples_sink_required', true);
  const cohorts = [];
  report.sample_artifacts = [];
  const measure = async (phase) => {
    const cohort = await driver.measureCohort(phase);
    check(cohort.phase === phase, 'measurement_phase_changed', true);
    for (const key of ['hardwareHash', 'fixtureHash', 'workloadHash', 'sourceHash', 'imagesHash']) {
      check(cohort[key] === config[key], 'measurement_provenance_changed', true);
    }
    check(cohort.observedReplicas === expectedReplicas(phase, config.scalingPeak),
      'cohort_replica_evidence_missing', true);
    report.sample_artifacts.push(await samples(cohort, cohorts.length));
    cohorts.push(cohort);
  };
  for (let i = 0; i < 3; i++) await measure('n1');
  report.calibration = calibrateNoise(cohorts, requiredRoutes);
  if (baselineOnly) {
    // Baseline observations are useful even when a comparison or a complete route is absent.
    // Keep missing rows and failed attempts visible without inventing N2/scaling deltas.
    report.performance = criteria.flatMap(({ metric, id, method, route, boundary, limitMs }) =>
      cohorts.map((cohort, window) => ({ metric, criterion_id: id, method, route,
        completion_boundary: boundary, phase: 'n1', window,
        ...summarize(cohort.samples?.[metric] ?? []), limit_ms: limitMs,
        delta_ms: null, delta_percent: null, noise_ms: report.calibration[metric].allowanceMs,
        status: 'BLOCKED', code: 'matched_n2_scaling_not_measured' })));
    await checkpoint(report);
    for (const cohort of cohorts) {
      for (const { metric } of criteria) {
        const values = cohort.samples?.[metric] ?? [];
        check(values.length >= 100, 'baseline_samples_incomplete', true);
        check(values.every((value) => value.ok), 'baseline_request_failed');
        if (Object.hasOwn(userLimits, metric)) {
          check(values.every((value) => value.rendered === true), 'rendered_completion_not_proven', true);
          check(values.every((value) => value.elapsedMs <= userLimits[metric]), 'baseline_latency_limit_exceeded');
        }
      }
    }
    return { captured_n1_windows: cohorts.length, comparison_qualified: false };
  }
  await checkpoint(report);
  for (const phase of ['n2', 'scaling']) await measure(phase);
  report.performance = compareCohorts(cohorts, requiredRoutes, 100, report.calibration,
    { scalingPeak: config.scalingPeak });
  check(!report.performance.some((row) => row.status === 'BLOCKED'), 'performance_measurement_unstable', true);
  check(report.performance.every((row) => row.status === 'PASS'), 'performance_regression');
  return { rows: report.performance.length };
}
