import { validateProviderStart } from './provider-start-schema.mjs';

const stats = (values) => {
  if (!values.length) return { n: 0, p95_ms: null, max_ms: null };
  const sorted = [...values].sort((a, b) => a - b);
  return { n: values.length, p95_ms: sorted[Math.ceil(sorted.length * 0.95) - 1], max_ms: sorted.at(-1) };
};
const intervalStats = (values) => ({ lower: stats(values.map((s) => s.lower_ms)), upper: stats(values.map((s) => s.upper_ms)) });

function reduceCohort(row, workload) {
  const m = row.measurement, samples = m.samples, reasons = new Set(), lags = [], acceptance = [], completion = [], ages = [];
  const denominator = (key) => samples.filter((s) => (s[key] ?? null) !== null).length;
  const counts = { offered: denominator('offered_ns'), admitted: samples.filter((s) => s.admitted).length,
    executor_started: denominator('executor_started_ns'), first_started: denominator('first_post_ns'),
    accepted: denominator('accepted_ns'), completed: denominator('completed_ns'),
    expired_before_start: denominator('expired_before_start_ns'),
    expired_after_post: denominator('expired_after_post_ns'),
    post_attempts: samples.reduce((n, s) => n + s.post_attempts, 0) };
  if (!m.measurement_enabled) reasons.add('measurement_disabled');
  if (m.overflow || m.window_exceeded) reasons.add('observation_bound_exceeded');
  if (workload.mode === 'smoke') reasons.add('smoke_is_not_full_workload');
  if (row.scheduler_aborted || row.scheduler_recovery_failed) reasons.add('scheduler_incomplete');
  if (!row.drained || m.active_executors) reasons.add('observation_not_drained');
  if (row.offer_window_ns < workload.offer_seconds * 1e9) reasons.add('offer_duration_incomplete');
  const offers = samples.filter((s) => s.offered_ns !== null).map((s) => s.offered_ns).sort((a, b) => a - b);
  const maxOfferDelay = Math.max(0, ...offers.map((at, i) => at - row.offer_start_ns - (workload.events_per_second ? i * 500e6 : 0)));
  if (offers.some((at, i) => at < row.offer_start_ns + (workload.events_per_second ? i * 500e6 : 0))
    || maxOfferDelay > 500e6) reasons.add('offered_schedule_not_qualified');
  if (Object.values(counts).slice(0, 6).some((n) => n !== row.expected_offered)) reasons.add('denominator_incomplete');
  for (const s of samples) {
    if (s.unknown) reasons.add(s.unknown);
    if (s.target_count !== 1) reasons.add('single_target_required');
    if (s.cancellations) reasons.add('executor_cancelled');
    if (s.commit_lower_ns === null || s.admission_confirmed_ns === null) {
      reasons.add('admission_interval_unknown');
      continue;
    }
    const at = s.first_post_ns ?? m.window_ns;
    const interval = { lower_ms: Math.max(0, at - s.admission_confirmed_ns - m.clock_resolution_ns) / 1e6,
      upper_ms: (at - s.commit_lower_ns + m.clock_resolution_ns) / 1e6 };
    if (s.first_post_ns === null) {
      if (s.admitted && s.post_attempts === 0 && s.unknown === null
        && s.expired_before_start_ns === null) ages.push(interval);
      reasons.add('first_write_unknown');
    } else {
      lags.push(interval);
      if (s.accepted_ns !== null && s.accepted_attempt_post_ns !== null) {
        acceptance.push((s.accepted_ns - s.accepted_attempt_post_ns) / 1e6);
      } else if ((s.expired_after_post_ns ?? null) === null) reasons.add('http_acceptance_unknown');
      if (s.accepted_ns !== null && s.completed_ns !== null) completion.push((s.completed_ns - s.accepted_ns) / 1e6);
    }
  }
  const lag = intervalStats(lags);
  const p95Breached = lags.filter((s) => s.lower_ms > 2000).length
    > row.expected_offered - Math.ceil(row.expected_offered * 0.95);
  if (counts.expired_after_post > 0) reasons.add('expired_after_post');
  const failed = counts.expired_before_start > 0 || counts.expired_after_post > 0 || p95Breached || lag.lower.max_ms > 10000;
  if (!lags.length || lag.upper.p95_ms > 2000 || lag.upper.max_ms > 10000) reasons.add('lag_not_proven_within_limit');
  return {
    cohort: row.cohort, status: failed ? 'FAIL' : reasons.size ? 'BLOCKED' : 'PASS',
    reasons: [...reasons].sort(), counts, expected_offered: row.expected_offered,
    lag_ms: lag, lag_population: 'observed_first_starts_with_admission_interval', limit_ms: { p95: 2000, max: 10000 },
    accepting_post_to_acceptance_ms: stats(acceptance), acceptance_to_terminal_ms: stats(completion),
    oldest_unstarted_ms: counts.admitted - counts.first_started - counts.expired_before_start === 0 ? null
      : ages.length === counts.admitted - counts.first_started - counts.expired_before_start
        ? { lower: Math.max(...ages.map((s) => s.lower_ms)), upper: Math.max(...ages.map((s) => s.upper_ms)) }
        : 'UNKNOWN',
    scheduler: { active: m.active_executors, peak: m.peak_executors, attempts: m.executor_attempts,
      average_active: m.executor_busy_ns / m.window_ns },
    offered_schedule: { max_lateness_ms: maxOfferDelay / 1e6, allowed_lateness_ms: 500 },
    clock: { unit: 'ns', domain: m.process, resolution_ns: m.clock_resolution_ns,
      admission_boundary: 'before_root_insert_through_successful_readback', candidate_time_unit: 'integer_seconds_only' },
  };
}

export function reduceProviderStart(evidence) {
  const e = validateProviderStart(evidence);
  const cohorts = e.payload.cohorts.map((row) => reduceCohort(row, e.workload));
  return { schema_version: 1, scope: 'local_synthetic_only', candidate: e.candidate,
    window: { started_at: e.started_at, ended_at: e.ended_at }, workload: e.workload,
    status: cohorts.some((c) => c.status === 'FAIL') ? 'FAIL' : cohorts.some((c) => c.status === 'BLOCKED') ? 'BLOCKED' : 'PASS',
    cohorts, real_load_canary_acceptance: { status: 'BLOCKED', code: 'not_executed' },
    limitations: ['loopback_record_fixture_is_not_mongo_durability', 'isolated_synthetic_provider_per_execution',
      'cold_and_media_are_preparation_proxies', 'fixed_wall_clock_does_not_qualify_real_deadlines',
      'no_deployed_image_or_real_provider', 'no_capacity_or_performance_improvement_claim'] };
}
