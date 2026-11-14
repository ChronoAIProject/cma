import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { check, finite, hash, isHash } from './checks.mjs';
import { catalogSha256, criteria } from './catalog.mjs';

export const scenarioNames = ['cross_replica', 'am_mid_stream', 'am_mid_command', 'eg_inbound_loss',
  'hpa_up_down', 'users_100', 'deterministic_contract', 'replica_sweep', 'am_101_trials',
  'eg_10_trials', 'eg_partition_load', 'performance'];

export async function saveSamples(directory, cohort, index) {
  check(Number.isSafeInteger(index) && index >= 0, 'invalid_cohort_index');
  check(['n1', 'n2', 'scaling', 'fault', 'rollout'].includes(cohort.phase), 'invalid_cohort_phase');
  const value = { phase: cohort.phase, index, catalog_sha256: catalogSha256, samples: {} };
  check([1, 2, '2-5-2'].includes(cohort.observedReplicas), 'cohort_replica_evidence_missing', true);
  value.observedReplicas = cohort.observedReplicas;
  for (const field of ['startedAt', 'finishedAt']) {
    check(typeof cohort[field] === 'string' && Number.isFinite(Date.parse(cohort[field])),
      'cohort_timestamps_missing', true);
    value[field] = new Date(cohort[field]).toISOString();
  }
  check(Date.parse(value.finishedAt) >= Date.parse(value.startedAt), 'cohort_window_invalid');
  for (const field of ['hardwareHash', 'fixtureHash', 'workloadHash', 'sourceHash', 'imagesHash']) {
    check(isHash(cohort[field]), 'cohort_provenance_missing', true);
    value[field] = cohort[field];
  }
  for (const { metric } of criteria) {
    const samples = cohort.samples?.[metric] ?? [];
    check(Array.isArray(samples), 'invalid_sample_set');
    value.samples[metric] = samples.map((sample) => {
      finite(sample.elapsedMs);
      check(typeof sample.ok === 'boolean', 'sample_outcome_required');
      // Project an allowlist: a driver may hold private bodies/IDs in memory.
      return { elapsedMs: sample.elapsedMs, ok: sample.ok, rendered: sample.rendered === true };
    });
  }
  await mkdir(directory, { recursive: true, mode: 0o700 });
  const file = `cohort-${index}-${cohort.phase}.json`;
  const bytes = JSON.stringify(value) + '\n';
  await writeFile(join(directory, file), bytes, { mode: 0o600, flag: 'wx' });
  return { file, sha256: hash(bytes) };
}

export function missingPerformance(code = 'not_run') {
  return criteria.flatMap(({ metric, id, method, route, boundary, limitMs }) =>
    ['n1', 'n2', 'scaling'].map((phase) => ({ metric, criterion_id: id, method, route,
      completion_boundary: boundary, phase, n: 0, failed: 0, completed: 0,
      p50_ms: null, p95_ms: null, p99_ms: null, max_ms: null,
      delta_ms: null, delta_percent: null, noise_ms: null, limit_ms: limitMs,
      status: 'BLOCKED', code })));
}

export async function saveReport(directory, report) {
  await mkdir(directory, { recursive: true, mode: 0o700 });
  report.catalog_sha256 = catalogSha256;
  if (!report.performance?.length) report.performance = missingPerformance();
  const json = JSON.stringify(report, null, 2) + '\n';
  await writeFile(join(directory, 'results.json'), json, { mode: 0o600 });
  const lines = ['# Local cma-scale results', '', `Overall: **${report.status}**. Environment: local.`, '',
    '| Scenario | Result | Evidence / blocker |', '|---|---|---|',
    ...report.scenarios.map((r) => `| ${r.scenario} | ${r.status} | ${r.code ?? r.evidence_hash ?? 'see results.json'} |`),
    '', 'A unit-test pass is not a local-cluster acceptance pass. Failures and missing prerequisites remain in the matrix.', '',
    `results.json SHA-256: ${hash(json)}`, ''];
  if (report.performance?.length) {
    lines.push('| Criterion / metric | Phase | N | Failed | p50 ms | p95 ms | p99 ms | Delta ms | Delta % | Noise ms | A3 class | Allowance ms | Result |',
      '|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|---:|---|');
    const fmt = (n) => n === null ? 'unmeasured' : n.toFixed(2);
    for (const r of report.performance) lines.push(`| ${r.criterion_id ?? r.metric}: ${r.route ? `${r.method} ${r.route}` : r.metric} | ${r.phase} | ${r.n} | ${r.failed} | ${fmt(r.p50_ms)} | ${fmt(r.p95_ms)} | ${fmt(r.p99_ms)} | ${fmt(r.delta_ms)} | ${fmt(r.delta_percent)} | ${fmt(r.noise_ms)} | ${r.tolerance_class ?? 'unclassified'} | ${fmt(r.tolerance_ms ?? null)} | ${r.status} |`);
  }
  await writeFile(join(directory, 'RESULTS.md'), lines.join('\n') + '\n', { mode: 0o600 });
}
