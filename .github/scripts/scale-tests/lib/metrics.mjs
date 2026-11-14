import { check, finite, isHash } from './checks.mjs';
import { stats } from '../timing/lib/core.mjs';
import { deltaInterval } from './confidence.mjs';
import { userLimits, criterionFor } from './catalog.mjs';
import { performanceClass, p95Allowance } from './performance-policy.mjs';
import { expectedReplicas } from './replicas.mjs';
export { userLimits } from './catalog.mjs';

// Same nearest-rank definition as prod-verifier-02 timing-harness/lib/core.mjs.
// Failures remain in the denominator and always prevent acceptance.
export function summarize(samples) {
  check(Array.isArray(samples), 'samples_required');
  for (const sample of samples) {
    finite(sample.elapsedMs);
    check(typeof sample.ok === 'boolean', 'sample_outcome_required');
  }
  const values = samples.filter((x) => x.ok).map((x) => x.elapsedMs).sort((a, b) => a - b);
  const timing = stats(samples.map((s) => ({ outcome: s.ok ? 'complete' : 'failed', elapsed_ms: s.elapsedMs })));
  return { n: samples.length, completed: values.length, failed: samples.length - values.length,
    p50_ms: timing.p50_ms, p95_ms: timing.p95_ms,
    p99_ms: values.length >= 1000 ? values[Math.ceil(0.99 * values.length) - 1] : null,
    max_ms: timing.max_ms };
}

export function calibrateNoise(baselines, requiredRoutes, { maxNoiseMs = 0 } = {}) {
  check(baselines.length >= 3 && baselines.every((c) => c.phase === 'n1'), 'three_baselines_required', true);
  finite(maxNoiseMs);
  return Object.fromEntries([...Object.keys(userLimits), ...requiredRoutes].map((metric) => {
    const summaries = baselines.map((b) => summarize(b.samples?.[metric] ?? []));
    if (!summaries.every((s) => s.n >= 100 && s.failed === 0)) {
      return [metric, { allowanceMs: null, stable: false, code: 'baseline_samples_incomplete' }];
    }
    const noise = Math.max(...summaries.map((s) => s.p95_ms)) - Math.min(...summaries.map((s) => s.p95_ms));
    return [metric, { allowanceMs: noise, stable: noise <= maxNoiseMs }];
  }));
}

export function compareCohorts(cohorts, requiredRoutes, minimumSamples = 100, calibration, { scalingPeak = 4 } = {}) {
  check(Array.isArray(requiredRoutes) && requiredRoutes.length > 0
    && new Set(requiredRoutes).size === requiredRoutes.length, 'route_inventory_required', true);
  check(Number.isSafeInteger(minimumSamples) && minimumSamples >= 100, 'insufficient_sample_budget', true);
  check(cohorts.length >= 5, 'repeated_baseline_required', true);
  const baseline = cohorts.filter((c) => c.phase === 'n1');
  check(baseline.length >= 3 && cohorts.some((c) => c.phase === 'n2')
    && cohorts.some((c) => c.phase === 'scaling'), 'comparison_cohort_missing', true);
  for (const cohort of cohorts) {
    check(['n1', 'n2', 'scaling'].includes(cohort.phase), 'invalid_cohort_phase');
    for (const key of ['hardwareHash', 'fixtureHash', 'workloadHash', 'sourceHash', 'imagesHash']) {
      check(isHash(cohort[key]) && cohort[key] === cohorts[0][key], 'cohort_provenance_mismatch', true);
    }
    check(cohort.observedReplicas === expectedReplicas(cohort.phase, scalingPeak),
      'cohort_replica_evidence_missing', true);
  }
  const rows = [];
  for (const metric of [...Object.keys(userLimits), ...requiredRoutes]) {
    check(/^[a-zA-Z0-9_/:.-]{1,160}$/.test(metric), 'invalid_metric_label');
    const criterion = criterionFor(metric);
    check(criterion, 'performance_class_missing', true);
    const classification = performanceClass('catalog', criterion.id);
    const stats = cohorts.map((cohort) => {
      const samples = cohort.samples?.[metric] ?? [];
      if (Object.hasOwn(userLimits, metric)) {
        check(samples.every((s) => s.rendered === true), 'rendered_completion_not_proven', true);
      }
      return summarize(samples);
    });
    const baselines = cohorts.flatMap((c, i) => c.phase === 'n1' ? [stats[i]] : []);
    const completeBaselines = baselines.every((s) => s.n >= minimumSamples && s.failed === 0);
    const base = completeBaselines ? Math.min(...baselines.map((s) => s.p95_ms)) : null;
    const positiveBaseline = base !== null && base > 0;
    const registration = calibration ?? calibrateNoise(baseline, requiredRoutes);
    const noise = registration[metric]?.allowanceMs;
    const calibrated = registration[metric]?.stable === true && Number.isFinite(noise) && noise >= 0;
    const baselineIndex = cohorts.findIndex((c, i) => c.phase === 'n1' && stats[i].p95_ms === base);
    for (let i = 0; i < cohorts.length; i++) {
      const value = stats[i];
      const limit = userLimits[metric] ?? null;
      const sufficient = completeBaselines && positiveBaseline && value.n >= minimumSamples;
      const interval = sufficient && value.failed === 0 ? deltaInterval(
        cohorts[baselineIndex].samples[metric].map((s) => s.elapsedMs),
        cohorts[i].samples[metric].map((s) => s.elapsedMs)) : null;
      const allowance = sufficient && calibrated ? p95Allowance({ kind: 'catalog', id: criterion.id }, base,
        { noiseMs: noise, noiseQualified: calibrated }) : null;
      const pass = sufficient && calibrated && value.failed === 0 && interval.upper_ms <= allowance.allowance_ms
        && (limit === null || value.max_ms <= limit);
      rows.push({ metric, ...(criterion ? { criterion_id: criterion.id, method: criterion.method,
        route: criterion.route, completion_boundary: criterion.boundary } : {}),
        tolerance_class: classification.class, tolerance_context: classification.context,
        class_table_sha256: classification.class_table_sha256,
        tolerance_ms: allowance?.allowance_ms ?? null,
        optimization_note: pass && allowance.aim_ms !== null && interval.upper_ms > allowance.aim_ms
          ? 'ux_above_five_percent_aim_within_ten_percent_cap' : null,
        phase: cohorts[i].phase, observedReplicas: cohorts[i].observedReplicas, ...value, baseline_p95_ms: base,
        noise_ms: noise ?? null, delta_ms: base === null || value.p95_ms === null ? null : value.p95_ms - base,
        delta_percent: base === null || base === 0 || value.p95_ms === null ? null : 100 * (value.p95_ms - base) / base,
        confidence_interval: interval, limit_ms: limit,
        ...(['create', 'create_desktop'].includes(metric) ? {
          breaches_5s: (cohorts[i].samples[metric] ?? []).filter((s) => s.elapsedMs > 5000).length } : {}),
        ...(!sufficient ? { code: positiveBaseline ? 'metric_samples_missing' : 'positive_baseline_missing' } : {}),
        status: sufficient && calibrated ? pass ? 'PASS' : 'FAIL' : 'BLOCKED' });
    }
  }
  return rows;
}
