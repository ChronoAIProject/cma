import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { hash } from '../lib/checks.mjs';
import { workloadFor } from '../lib/provider-start-schema.mjs';
import { reduceProviderStart } from '../lib/provider-start.mjs';

function bind(e) {
  e.payload_sha256 = hash(JSON.stringify(e.payload));
  e.bindings = e.payload.cohorts.map((row) => ({ candidate: structuredClone(e.candidate),
    config_sha256: hash(JSON.stringify(row.config)), workload_sha256: hash(JSON.stringify(e.workload)), process: row.measurement.process }));
  return e;
}
export function fixture(mode = 'burst') {
  const workload = workloadFor(mode);
  const cohorts = ['bound', 'cold'].flatMap((binding) => [false, true].map((media) => ({
    cohort: { binding, media }, mode, expected_offered: workload.events_per_cohort,
    offer_start_ns: 0, offer_window_ns: workload.offer_seconds * 1e9 + 100e6, drained: true, actual_posts: workload.events_per_cohort,
    scheduler_aborted: 0, scheduler_recovery_failed: false,
    config: { scheduler_concurrency: 8, queue_capacity: 512, preparation_delay_ms: (binding === 'cold' ? 25 : 0) + (media ? 10 : 0),
      input_bytes: media ? 4096 : 32, observation_delay_ms: workload.observation_delay_ms,
      provider: 'isolated_loopback_per_execution', durable_store: 'loopback_record_fixture',
      wall_clock: 'fixed_synthetic_seconds', cohort_model: 'synthetic_preparation_delay_and_payload' },
    measurement: { schema_version: 1, measurement_enabled: true, clock: 'process_monotonic_ns', clock_resolution_ns: 1e6,
      process: randomUUID(), window_ns: (workload.offer_seconds + 20) * 1e9, capacity: 4096, overflow: false, window_exceeded: false,
      active_executors: 0, peak_executors: 8, executor_attempts: workload.events_per_cohort, executor_busy_ns: 10e9,
      samples: Array.from({ length: workload.events_per_cohort }, (_, sample) => {
        const offset = workload.events_per_second ? sample * 500e6 : 0;
        return { sample, offered_ns: offset + 1e6,
          candidate_admitted_at_seconds: 1_800_000_000, commit_lower_ns: offset + 2e6, admission_confirmed_ns: offset + 3e6, admitted: true,
          target_count: 1, executor_started_ns: offset + 4e6, first_post_ns: offset + 10e6, post_attempts: 1,
          accepted_ns: offset + 12e6, accepted_attempt_post_ns: offset + 10e6,
          completed_ns: offset + 13e9, expired_before_start_ns: null, cancellations: 0, unknown: null };
      }) },
  })));
  return bind({ schema_version: 1, candidate: { source_sha: 'a'.repeat(40), executable_sha256: 'b'.repeat(64),
    image: 'not_deployed_local_test_executable' }, started_at: '2026-11-20T00:00:00.000Z',
  ended_at: '2026-11-20T00:31:00.000Z', workload, payload: { schema_version: 1, scope: 'local_synthetic_only', cohorts } });
}
const first = (e) => e.payload.cohorts[0];
const mutate = (edit) => { const e = fixture(); edit(first(e), e); return bind(e); };
const versionTwo = (e) => {
  for (const row of e.payload.cohorts) {
    row.measurement.schema_version = 2;
    for (const sample of row.measurement.samples) sample.expired_after_post_ns = null;
  }
  return e;
};

test('version two distinguishes expiry after a refused POST without changing original denominators', () => {
  assert.equal(reduceProviderStart(bind(versionTwo(fixture()))).status, 'PASS');
  const e = versionTwo(fixture());
  const s = first(e).measurement.samples[0];
  s.accepted_ns = s.accepted_attempt_post_ns = s.completed_ns = null;
  s.expired_after_post_ns = 11e9;
  const report = reduceProviderStart(bind(e));
  const c = report.cohorts[0];
  assert.equal(report.status, 'FAIL');
  assert.deepEqual(c.counts, { offered: 100, admitted: 100, executor_started: 100, first_started: 100,
    accepted: 99, completed: 99, expired_before_start: 0, expired_after_post: 1, post_attempts: 100 });
  assert.equal(c.lag_ms.upper.max_ms, 9);
  assert.equal(c.oldest_unstarted_ms, null);
  assert.ok(c.reasons.includes('expired_after_post'));
  assert.ok(!c.reasons.includes('http_acceptance_unknown'));
});

test('expiry with missing original observation stays BLOCKED without inventing either expiry class', () => {
  for (const knownAdmission of [false, true]) {
  const e = versionTwo(fixture());
  const r = first(e), s = r.measurement.samples[0];
  if (!knownAdmission) s.commit_lower_ns = s.admission_confirmed_ns = null;
  s.first_post_ns = null;
  s.accepted_ns = s.accepted_attempt_post_ns = s.completed_ns = null;
  s.post_attempts = 0; r.actual_posts--;
  s.unknown = knownAdmission ? 'first_post_observation_missing' : 'retained_or_uncertain_admission';
  const c = reduceProviderStart(bind(e)).cohorts[0];
  assert.equal(c.status, 'BLOCKED');
  assert.equal(c.counts.expired_before_start, 0);
  assert.equal(c.counts.expired_after_post, 0);
  assert.ok(c.reasons.includes(s.unknown));
  }
});

test('a proved retry write with an unknown first POST stays out of lag percentiles', () => {
  const e = versionTwo(fixture());
  const sample = first(e).measurement.samples[0];
  sample.first_post_ns = null;
  sample.unknown = 'first_post_observation_missing';
  const c = reduceProviderStart(bind(e)).cohorts[0];
  assert.equal(c.status, 'BLOCKED');
  assert.equal(c.counts.first_started, 99);
  assert.equal(c.counts.post_attempts, 100);
  assert.equal(c.lag_ms.upper.n, 99);
  assert.equal(c.oldest_unstarted_ms, 'UNKNOWN');
});

test('both schema versions reject contradictory expiry and version two requires exact fields', () => {
  for (const version of [1, 2]) {
    const e = version === 1 ? fixture() : versionTwo(fixture());
    first(e).measurement.samples[0].expired_before_start_ns = 11e9;
    assert.throws(() => reduceProviderStart(bind(e)), /provider_start_expiry_after_post/);
  }
  for (const edit of [
    (s) => { s.accepted_ns = 12e6; },
    (s) => { s.completed_ns = 12e9; },
    (s) => { s.first_post_ns = null; s.post_attempts = 0; },
    (s) => { s.expired_after_post_ns = 1; },
    (s) => { delete s.expired_after_post_ns; },
    (s) => { s.expiry = 'unrecognized'; },
  ]) {
    const e = versionTwo(fixture()), s = first(e).measurement.samples[0];
    s.accepted_ns = s.accepted_attempt_post_ns = s.completed_ns = null;
    s.expired_after_post_ns = 11e9;
    edit(s);
    assert.throws(() => reduceProviderStart(bind(e)));
  }
});

test('separates cohorts, socket lag, acceptance and terminal; local PASS leaves canary BLOCKED', () => {
  const report = reduceProviderStart(fixture());
  assert.equal(report.status, 'PASS'); assert.equal(report.cohorts.length, 4);
  assert.deepEqual(report.cohorts[0].lag_ms, { lower: { n: 100, p95_ms: 6, max_ms: 6 }, upper: { n: 100, p95_ms: 9, max_ms: 9 } });
  assert.equal(report.cohorts[0].accepting_post_to_acceptance_ms.p95_ms, 2);
  assert.equal(report.cohorts[0].acceptance_to_terminal_ms.p95_ms, 12988);
  assert.equal(report.real_load_canary_acceptance.status, 'BLOCKED');
});
test('synthetic p95 above 2s and isolated max above 10s independently FAIL', () => {
  for (const [quantity, ns] of [[100, 3e9], [1, 11e9]]) {
    const e = mutate((r) => r.measurement.samples.slice(0, quantity).forEach((s) => {
      s.first_post_ns = s.accepted_attempt_post_ns = ns; s.accepted_ns = ns + 1e6;
    }));
    assert.equal(reduceProviderStart(e).status, 'FAIL');
  }
});
test('insufficient clock precision straddling a threshold is BLOCKED', () => {
  const e = mutate((r) => { r.measurement.clock_resolution_ns = 3e9; });
  assert.equal(reduceProviderStart(e).status, 'BLOCKED');
});
test('started executor without a POST stays unknown with oldest-unstarted age', () => {
  const e = mutate((r) => {
    const s = r.measurement.samples[0];
    s.first_post_ns = s.accepted_ns = s.accepted_attempt_post_ns = s.completed_ns = null; s.post_attempts = 0; r.actual_posts--;
  });
  const report = reduceProviderStart(e);
  assert.equal(report.status, 'BLOCKED');
  assert.equal(report.cohorts[0].counts.executor_started, 100);
  assert.equal(report.cohorts[0].counts.first_started, 99);
  assert.ok(report.cohorts[0].oldest_unstarted_ms.lower > 19000);
});
test('lost acceptance, cancellation, restart and expired-before-start preserve distinct outcomes', () => {
  for (const kind of ['lost_acceptance', 'cancelled', 'restart', 'expired']) {
    const e = mutate((r) => {
      const s = r.measurement.samples[0];
      if (kind === 'lost_acceptance') s.accepted_ns = s.accepted_attempt_post_ns = s.completed_ns = null;
      if (kind === 'cancelled') s.cancellations = 1;
      if (kind === 'restart') {
        s.commit_lower_ns = s.admission_confirmed_ns = null; s.unknown = 'retained_or_uncertain_admission';
      }
      if (kind === 'expired') {
        s.first_post_ns = s.accepted_ns = s.accepted_attempt_post_ns = s.completed_ns = null;
        s.post_attempts = 0; s.expired_before_start_ns = 11e9; r.actual_posts--;
      }
    });
    assert.equal(reduceProviderStart(e).status, kind === 'expired' ? 'FAIL' : 'BLOCKED');
  }
});
test('retry attempts never increase admitted or first-start denominators', () => {
  const e = mutate((r) => { r.measurement.samples[0].post_attempts++; r.actual_posts++; r.measurement.executor_attempts++; });
  const c = reduceProviderStart(e).cohorts[0];
  assert.equal(c.counts.admitted, 100); assert.equal(c.counts.first_started, 100); assert.equal(c.counts.post_attempts, 101);
});
test('partial first-start data cannot prove an overall p95 breach from one slow sample', () => {
  const e = mutate((r) => {
    r.measurement.samples.forEach((s, i) => {
      if (i === 0) { s.first_post_ns = s.accepted_attempt_post_ns = 3e9; s.accepted_ns = 3.01e9; }
      else { s.first_post_ns = s.accepted_attempt_post_ns = s.accepted_ns = s.completed_ns = null; s.post_attempts = 0; }
    });
    r.actual_posts = 1;
  });
  assert.equal(reduceProviderStart(e).status, 'BLOCKED');
});
test('overflow, missing terminal, incomplete duration and smoke cannot PASS', () => {
  for (const edit of [(r) => { r.measurement.overflow = true; }, (r) => { r.measurement.window_exceeded = true; },
    (r) => { r.measurement.samples[0].completed_ns = null; }, (r) => { r.measurement.measurement_enabled = false; }]) {
    assert.equal(reduceProviderStart(mutate(edit)).status, 'BLOCKED');
  }
  assert.equal(reduceProviderStart(fixture('smoke')).status, 'BLOCKED');
  const e = fixture('sustained'); first(e).offer_window_ns = 1e9;
  assert.equal(reduceProviderStart(bind(e)).status, 'BLOCKED');
});
test('rejects missing fields, wrong units, duplicate data, inconsistent post counts and mixed candidates', () => {
  const edits = [
    (r) => { delete r.measurement.samples[0].first_post_ns; },
    (r) => { r.measurement.clock = 'wall_milliseconds'; },
    (r) => { r.measurement.samples[1].sample = 0; },
    (r) => { r.measurement.samples[0].first_post_ns = 0.5; },
    (r) => { r.actual_posts++; },
    (r, e) => { e.payload.cohorts[1] = structuredClone(r); },
    (r) => { r.config.scheduler_concurrency = 16; },
  ];
  for (const edit of edits) assert.throws(() => reduceProviderStart(mutate(edit)));
  const e = fixture(); e.bindings[0].candidate.source_sha = 'c'.repeat(40);
  assert.throws(() => reduceProviderStart(e), /mixed_provenance/);
  const tampered = fixture(); first(tampered).measurement.samples[0].first_post_ns++;
  assert.throws(() => reduceProviderStart(tampered), /payload_hash_mismatch/);
});
