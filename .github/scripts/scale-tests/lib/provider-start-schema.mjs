import { check, count, hash, isHash } from './checks.mjs';

export const workloadFor = (mode) => {
  check(['burst', 'sustained', 'smoke'].includes(mode), 'invalid_provider_start_mode');
  return { mode, targets_per_event: 1, events_per_cohort: mode === 'burst' ? 100 : mode === 'sustained' ? 3600 : 8,
    offer_seconds: mode === 'burst' ? 0 : mode === 'sustained' ? 1800 : 4,
    events_per_second: mode === 'burst' ? null : 2,
    observation_delay_ms: mode === 'burst' ? 12000 : 100 };
};
const fields = (value, names) => check(value && typeof value === 'object' && !Array.isArray(value)
  && Object.keys(value).sort().join(',') === [...names].sort().join(','), 'provider_start_fields_invalid');
const boolean = (value) => check(typeof value === 'boolean', 'provider_start_boolean_invalid');
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const stampFields = ['offered_ns', 'commit_lower_ns', 'admission_confirmed_ns', 'executor_started_ns',
  'first_post_ns', 'accepted_ns', 'accepted_attempt_post_ns', 'completed_ns', 'expired_before_start_ns'];

function sample(row, window, ids, version) {
  const stamps = version === 2 ? [...stampFields, 'expired_after_post_ns'] : stampFields;
  fields(row, ['sample', ...stamps, 'candidate_admitted_at_seconds', 'admitted', 'target_count',
    'post_attempts', 'cancellations', 'unknown']);
  count(row.sample);
  check(!ids.has(row.sample), 'duplicate_provider_start_sample');
  ids.add(row.sample);
  for (const key of stamps) {
    if (row[key] !== null) check(count(row[key]) <= window, 'provider_start_stamp_outside_window');
  }
  if (row.candidate_admitted_at_seconds !== null) count(row.candidate_admitted_at_seconds);
  boolean(row.admitted); count(row.target_count); count(row.post_attempts); count(row.cancellations);
  check(row.unknown === null || ['retained_or_uncertain_admission', 'single_target_required',
    'dispatch_refused_or_failed'].includes(row.unknown)
    || (version === 2 && row.unknown === 'first_post_observation_missing'), 'provider_start_unknown_reason_invalid');
  const unprovedFirstWrite = version === 2 && row.unknown !== null && row.first_post_ns === null && row.post_attempts > 0;
  check((row.first_post_ns !== null) === (row.post_attempts > 0) || unprovedFirstWrite,
    'provider_start_post_count_mismatch');
  check(row.accepted_attempt_post_ns === null || (row.accepted_ns !== null && row.post_attempts > 0),
    'provider_start_accepting_attempt_invalid');
  check(row.admitted || (row.commit_lower_ns === null && row.admission_confirmed_ns === null),
    'provider_start_admission_invalid');
  const ordered = (a, b) => check(row[a] === null || row[b] === null || row[a] <= row[b],
    'provider_start_timestamp_order');
  ordered('offered_ns', 'commit_lower_ns');
  ordered('commit_lower_ns', 'admission_confirmed_ns');
  ordered('commit_lower_ns', 'first_post_ns');
  ordered('executor_started_ns', 'first_post_ns');
  ordered('first_post_ns', 'accepted_ns');
  ordered('first_post_ns', 'accepted_attempt_post_ns');
  ordered('accepted_attempt_post_ns', 'accepted_ns');
  ordered('accepted_ns', 'completed_ns');
  ordered('admission_confirmed_ns', 'expired_before_start_ns');
  check(row.expired_before_start_ns === null || row.post_attempts === 0, 'provider_start_expiry_after_post');
  check(row.expired_before_start_ns === null || (row.admitted && row.commit_lower_ns !== null
    && row.admission_confirmed_ns !== null && row.unknown === null && row.accepted_ns === null
    && row.completed_ns === null), 'provider_start_expiry_without_original_observation');
  if (version === 2) {
    ordered('first_post_ns', 'expired_after_post_ns');
    check(row.expired_after_post_ns === null || (row.first_post_ns !== null && row.expired_before_start_ns === null
      && row.accepted_ns === null && row.completed_ns === null), 'provider_start_expiry_after_post_invalid');
  }
}

function measurement(m) {
  fields(m, ['schema_version', 'measurement_enabled', 'clock', 'clock_resolution_ns', 'process', 'window_ns',
    'capacity', 'overflow', 'window_exceeded', 'active_executors', 'peak_executors', 'executor_attempts',
    'executor_busy_ns', 'samples']);
  check([1, 2].includes(m.schema_version) && m.clock === 'process_monotonic_ns', 'provider_start_clock_or_version_invalid');
  check(typeof m.process === 'string' && /^[a-f0-9-]{36}$/.test(m.process), 'provider_start_process_missing');
  for (const key of ['measurement_enabled', 'overflow', 'window_exceeded']) boolean(m[key]);
  for (const key of ['window_ns', 'clock_resolution_ns', 'capacity', 'active_executors', 'peak_executors',
    'executor_attempts', 'executor_busy_ns']) count(m[key]);
  check(m.window_ns > 0 && m.clock_resolution_ns > 0, 'provider_start_precision_missing');
  check(m.capacity > 0 && m.capacity <= 8192 && Array.isArray(m.samples) && m.samples.length <= m.capacity,
    'provider_start_capacity_invalid');
  check(m.active_executors <= m.peak_executors && m.peak_executors <= m.executor_attempts
    && m.executor_busy_ns <= m.window_ns * m.peak_executors, 'provider_start_occupancy_invalid');
  const ids = new Set();
  for (const row of m.samples) sample(row, m.window_ns, ids, m.schema_version);
  check(m.samples.filter((s) => s.executor_started_ns !== null).length <= m.executor_attempts,
    'provider_start_executor_count_mismatch');
}

export function validateProviderStart(e) {
  fields(e, ['schema_version', 'candidate', 'started_at', 'ended_at', 'workload', 'payload', 'payload_sha256', 'bindings']);
  fields(e.candidate, ['source_sha', 'executable_sha256', 'image']);
  check(e.schema_version === 1 && /^[a-f0-9]{40}$/.test(e.candidate.source_sha)
    && isHash(e.candidate.executable_sha256) && e.candidate.image === 'not_deployed_local_test_executable',
  'provider_start_candidate_invalid');
  const utc = (value) => typeof value === 'string' && /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/.test(value)
    && Number.isFinite(Date.parse(value)) && new Date(value).toISOString() === value;
  check(utc(e.started_at) && utc(e.ended_at) && Date.parse(e.ended_at) >= Date.parse(e.started_at),
    'provider_start_utc_window_invalid');
  check(same(e.workload, workloadFor(e.workload?.mode)), 'provider_start_workload_mismatch');
  check(isHash(e.payload_sha256) && hash(JSON.stringify(e.payload)) === e.payload_sha256,
    'provider_start_payload_hash_mismatch');
  fields(e.payload, ['schema_version', 'scope', 'cohorts']);
  check(e.payload.schema_version === 1 && e.payload.scope === 'local_synthetic_only'
    && Array.isArray(e.payload.cohorts) && e.payload.cohorts.length === 4
    && Array.isArray(e.bindings) && e.bindings.length === 4, 'provider_start_cohorts_required');
  const cohorts = new Set(), processes = new Set();
  e.payload.cohorts.forEach((row, index) => {
    fields(row, ['cohort', 'mode', 'expected_offered', 'offer_start_ns', 'offer_window_ns', 'drained', 'actual_posts',
      'scheduler_aborted', 'scheduler_recovery_failed', 'config', 'measurement']);
    fields(row.cohort, ['binding', 'media']); boolean(row.cohort.media);
    check(['cold', 'bound'].includes(row.cohort.binding), 'provider_start_cohort_invalid');
    const name = `${row.cohort.binding}/${row.cohort.media ? 'media' : 'no-media'}`;
    check(!cohorts.has(name), 'duplicate_provider_start_cohort'); cohorts.add(name);
    check(row.mode === e.workload.mode && row.expected_offered === e.workload.events_per_cohort,
      'provider_start_workload_mismatch');
    count(row.offer_start_ns); count(row.offer_window_ns); count(row.actual_posts); count(row.scheduler_aborted);
    boolean(row.drained); boolean(row.scheduler_recovery_failed);
    const config = { scheduler_concurrency: 8, queue_capacity: 512,
      preparation_delay_ms: (row.cohort.binding === 'cold' ? 25 : 0) + (row.cohort.media ? 10 : 0),
      input_bytes: row.cohort.media ? 4096 : 32, observation_delay_ms: e.workload.observation_delay_ms,
      provider: 'isolated_loopback_per_execution', durable_store: 'loopback_record_fixture',
      wall_clock: 'fixed_synthetic_seconds', cohort_model: 'synthetic_preparation_delay_and_payload' };
    fields(row.config, Object.keys(config));
    check(Object.keys(config).every((k) => row.config[k] === config[k]), 'provider_start_config_mismatch');
    const binding = e.bindings[index];
    fields(binding, ['candidate', 'config_sha256', 'workload_sha256', 'process']);
    check(same(binding.candidate, e.candidate) && binding.config_sha256 === hash(JSON.stringify(row.config))
      && binding.workload_sha256 === hash(JSON.stringify(e.workload)) && binding.process === row.measurement?.process,
    'provider_start_mixed_provenance');
    measurement(row.measurement);
    check(!processes.has(row.measurement.process), 'duplicate_provider_start_process'); processes.add(row.measurement.process);
    check(row.actual_posts === row.measurement.samples.reduce((n, s) => n + s.post_attempts, 0),
      'provider_start_independent_post_count_mismatch');
    check(row.offer_start_ns + row.offer_window_ns <= row.measurement.window_ns, 'provider_start_offer_window_invalid');
    check(row.measurement.window_ns <= (Date.parse(e.ended_at) - Date.parse(e.started_at)) * 1e6 + row.measurement.clock_resolution_ns,
      'provider_start_utc_window_too_short');
  });
  return e;
}
