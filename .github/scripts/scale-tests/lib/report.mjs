import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { check, finite, hash, isHash, reconcile, safeFailure } from './checks.mjs';
import { catalogSha256, criteria } from './catalog.mjs';

export const scenarioNames = ['cross_replica', 'am_mid_stream', 'am_mid_command', 'eg_inbound_loss',
  'hpa_up_down', 'users_100', 'deterministic_contract', 'replica_sweep', 'am_101_trials',
  'eg_10_trials', 'eg_partition_load', 'performance'];

export const haScenarioNames = ['deterministic_contract', 'replica_sweep', 'am_101_trials', 'eg_10_trials', 'ha_evidence'];
export const sw04Selection = 'sw04-cancel';
export const sw04Scenario = 'sw04_cancel';
export const selections = ['all', 'baseline-n1', 'ha'];
export const selectedScenarioNames = (selection) => selection === 'ha' ? haScenarioNames
  : selection === 'baseline-n1' ? ['n1_capture'] : scenarioNames;

export function recordHaObservation(section, value) {
  const record = { section };
  record.request = {
    role: ['am', 'eg'].includes(value?.request?.role) ? value.request.role : null,
    mode: ['process', 'graceful', 'node'].includes(value?.request?.mode) ? value.request.mode : null,
    trial: Number.isSafeInteger(value?.request?.trial) && value.request.trial >= 0 ? value.request.trial : null,
  };
  // Only typed fields and opaque hashes may leave the driver's private evidence packet.
  for (const key of ['candidateHash', 'runHash', 'evidenceHash', 'trialHash', 'faultEvidenceHash',
    'lostProcessHash', 'lostIncarnationHash', 'lostNodeHash', 'homeHashBefore', 'homeHashAfter']) {
    record[key] = isHash(value?.[key]) ? value[key] : null;
  }
  for (const key of ['startedAtMs', 'finishedAtMs', 'attempt', 'lossStartedAtMs', 'sustainedSuccessAtMs',
    'faultStartedAtMs', 'faultObservedAtMs', 'drainStartedAtMs', 'recoveryMs', 'minimumConnectedUsers',
    'minimumOpenStreams', 'actualReadsPerSecond', 'actualTurnsPerMinute']) {
    record[key] = Number.isFinite(value?.[key]) && value[key] >= 0 ? value[key] : null;
  }
  for (const key of ['completed', 'cancelled', 'sustainedSuccess', 'processDeathObserved', 'nodeLossObserved']) {
    record[key] = typeof value?.[key] === 'boolean' ? value[key] : null;
  }
  for (const [key, allowed] of Object.entries({ role: ['am', 'eg'], scalingMode: ['manual', 'hpa'],
    faultKind: ['process', 'graceful', 'node', 'node_drain'], firstRunOutcome: ['PASS', 'FAIL', 'BLOCKED', 'CANCELLED'] })) {
    record[key] = allowed.includes(value?.[key]) ? value[key] : null;
  }
  for (const key of ['processHashes', 'clientHashes', 'affectedAcceptedHashes']) {
    record[key] = Array.isArray(value?.[key]) ? value[key].map((id) => isHash(id) ? id : null) : [];
  }
  record.replicas = Object.fromEntries(['am', 'eg'].map((role) => [role, {
    configured: Number.isSafeInteger(value?.[role]?.desired) ? value[role].desired : null,
    ready: Number.isSafeInteger(value?.[role]?.ready) ? value[role].ready : null,
    serving: Array.isArray(value?.[role]?.servingReplicaHashes)
      ? value[role].servingReplicaHashes.map((id) => isHash(id) ? id : null) : [],
  }]));
  try { record.reconciliation = { status: 'PASS', ...reconcile(value?.oracle) }; }
  catch (error) { record.reconciliation = safeFailure(error); }
  return record;
}

export async function saveSamples(directory, cohort, index) {
  check(Number.isSafeInteger(index) && index >= 0, 'invalid_cohort_index');
  check(['n1', 'n2', 'scaling', 'fault', 'rollout'].includes(cohort.phase), 'invalid_cohort_phase');
  const value = { phase: cohort.phase, index, catalog_sha256: catalogSha256, samples: {} };
  check([1, 2, '2-4-2', '2-5-2'].includes(cohort.observedReplicas), 'cohort_replica_evidence_missing', true);
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
  const sw04 = report.selection === sw04Selection;
  if (!sw04 && report.selection !== 'ha' && !report.performance?.length) report.performance = missingPerformance();
  const json = JSON.stringify(report, null, 2) + '\n';
  await writeFile(join(directory, 'results.json'), json, { mode: 0o600 });
  const summary = sw04
    ? [`SW04 Cancel fixture: **${report.scenarios[0].status}**.`,
      `Overall scale qualification: **${report.track_status}**. Environment: local.`]
    : [`Overall: **${report.status}**. Environment: local.`];
  const lines = ['# Local cma-scale results', '', ...summary, '',
    '| Scenario | Result | Evidence / blocker |', '|---|---|---|',
    ...report.scenarios.map((r) => `| ${r.scenario} | ${r.status} | ${r.code ?? r.evidence_hash ?? 'see results.json'} |`),
    '', 'A unit-test pass is not a local-cluster acceptance pass. Failures and missing prerequisites remain in the matrix.', '',
    `results.json SHA-256: ${hash(json)}`, ''];
  if (report.ha) {
    lines.push(`HA component evidence: **${report.ha.status}**. Overall qualification remains **${report.status}**.`, '',
      '| HA evidence section | Index | Result | Blocker |', '|---|---:|---|---|',
      ...(report.ha.failures ?? []).map((f) => `| ${f.section} | ${f.index ?? 'all'} | ${f.status} | ${f.code} |`), '');
  }
  if (report.performance?.length) {
    lines.push('| Criterion / metric | Phase | N | Failed | p50 ms | p95 ms | p99 ms | Delta ms | Delta % | Noise ms | A3 class | Allowance ms | Result |',
      '|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|---:|---|');
    const fmt = (n) => n === null ? 'unmeasured' : n.toFixed(2);
    for (const r of report.performance) lines.push(`| ${r.criterion_id ?? r.metric}: ${r.route ? `${r.method} ${r.route}` : r.metric} | ${r.phase} | ${r.n} | ${r.failed} | ${fmt(r.p50_ms)} | ${fmt(r.p95_ms)} | ${fmt(r.p99_ms)} | ${fmt(r.delta_ms)} | ${fmt(r.delta_percent)} | ${fmt(r.noise_ms)} | ${r.tolerance_class ?? 'unclassified'} | ${fmt(r.tolerance_ms ?? null)} | ${r.status} |`);
  }
  await writeFile(join(directory, 'RESULTS.md'), lines.join('\n') + '\n', { mode: 0o600 });
}
