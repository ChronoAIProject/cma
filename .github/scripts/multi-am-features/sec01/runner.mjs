import {randomUUID} from 'node:crypto';
import {cells, CATEGORIES, STATES} from './cells.mjs';
import {aggregate, evaluateProbe, digest, sha} from './evaluate.mjs';

const check = (value, code) => { if (!value) throw Error(code); };
const safe = value => typeof value === 'string' && /^[a-z][a-z0-9_.-]{0,120}$/.test(value);
const blocked = (cell, reason, runId) => ({schema: 'sec01-cell-result-v1', id: cell.id,
  category: cell.category, state: cell.state, direction: cell.direction, status: 'BLOCKED',
  reason, run_id: runId, attempts: 0, pass: 0, fail: 0, blocked: 1});
const call = async (fixture, name, args) => {
  if (typeof fixture?.[name] !== 'function') throw Error('fixture_hook_missing_' + name);
  return fixture[name](args);
};

export function probeSpecs(cell, binding) {
  const base = {cell_id: cell.id, state: cell.state, direction: cell.direction,
    owner_tenant: cell.owner_tenant, foreign_tenant: cell.foreign_tenant};
  const category = CATEGORIES[cell.category];
  const specs = [];
  for (const probe of category.probes) specs.push({
    ...base, id: `${cell.id}:${probe}`, kind: cell.category === 'credential' ? 'credential' : ['foreign_home', 'foreign_workspace', 'foreign_volume',
      'traversal', 'symlink', 'proc_root_alias', 'retired_recycled', 'host_mount', 'projected_token',
      'host_mount_token'].includes(probe) ? 'file' : 'tcp', scenario: probe, expect: 'deny',
    target_id: `${cell.foreign_tenant}:${probe}`, denial_code: cell.category === 'credential'
      ? 'credential_not_authorized' : cell.category === 'file' ? 'file_not_found' : 'network_denied',
    owner_control: binding.targets?.[probe] ?? null,
  });
  for (const positive of category.positive) specs.push({
    ...base, id: `${cell.id}:positive:${positive}`, kind: cell.category === 'credential' ? 'credential' : 'http',
    scenario: positive, operation: positive, expect: 'allow', expected_sha256: binding.positives?.[positive]?.sha256,
    expected_bytes: binding.positives?.[positive]?.bytes,
  });
  return specs;
}

function validateRuntimeBinding(cell, binding, runId, candidate = {}) {
  check(binding?.schema === 'sec01-runtime-binding-v1', 'runtime_binding_schema');
  check(binding.run_id === runId && binding.cell_id === cell.id
    && binding.state === cell.state && binding.direction === cell.direction,
    'runtime_binding_context');
  check(/^[0-9a-f]{40}$/.test(binding.source_sha ?? '')
    && /^@sha256:[0-9a-f]{64}$/.test(binding.image_digest ?? ''), 'candidate_binding_missing');
  if (candidate.source) check(binding.source_sha === candidate.source, 'candidate_source_mismatch');
  if (candidate.image) check(binding.image_digest === candidate.image, 'candidate_image_mismatch');
  check(safe(binding.runtime_id) && safe(binding.incarnation), 'runtime_identity_missing');
  check(['a', 'b'].every(am => safe(binding.ams?.[am]?.instance_id)
    && safe(binding.ams[am].pod_uid) && binding.ams[am].ready === true), 'two_am_binding_missing');
  check(binding.ams.a.instance_id !== binding.ams.b.instance_id
    && binding.ams.a.pod_uid !== binding.ams.b.pod_uid, 'two_am_processes_not_distinct');
  check(binding.tenant?.owner === cell.owner_tenant && binding.tenant?.foreign === cell.foreign_tenant,
    'tenant_binding_missing');
  check(sha(binding.evidence_sha256), 'runtime_binding_evidence_missing');
  const lifecycle = binding.lifecycle;
  check(lifecycle?.state === cell.state && sha(lifecycle.evidence_sha256), 'lifecycle_evidence_missing');
  if (cell.state === 'cold') check(lifecycle.new_runtime === true && lifecycle.tenant_history === false,
    'cold_runtime_not_new');
  if (cell.state === 'warm') check(lifecycle.reused_after_retirement === true && lifecycle.sanitized === true,
    'warm_reuse_not_proved');
  if (cell.state === 'resumed') check(lifecycle.paused === true && lifecycle.authorization_changed_while_paused === true
    && lifecycle.restored_normally === true, 'resume_transition_not_proved');
  check(Array.isArray(binding.serving) && binding.serving.length >= 2
    && new Set(binding.serving.map(x => x.am)).size === 2
    && binding.serving.every(x => x.verdict === 'PASS' && sha(x.evidence_sha256)), 'am_serving_not_proved');
}

function validateObservation(observation, spec, binding, context) {
  check(observation?.schema === 'sec01-observation-v1', 'observation_schema');
  check(observation.cell_id === spec.cell_id && observation.id === spec.id
    && observation.state === spec.state && observation.direction === spec.direction,
  'observation_context');
  check(observation.run_id === context.run_id && observation.runtime_id === binding.runtime_id
    && observation.incarnation === binding.incarnation && safe(observation.am), 'observation_binding');
  check(sha(observation.evidence_sha256), 'observation_evidence_missing');
  check(observation.tenant?.owner === context.owner_tenant
    && observation.tenant?.foreign === context.foreign_tenant, 'observation_tenant_binding');
  return observation;
}

async function runCell(fixture, cell, runId, record, candidate) {
  const started = new Date().toISOString();
  let binding, cleanup;
  try {
    binding = await call(fixture, 'prepareCell', {cell, run_id: runId, ...candidate});
    cleanup = binding?.cleanup;
    validateRuntimeBinding(cell, binding, runId, candidate);
    check(typeof cleanup === 'function', 'cleanup_hook_missing');
    const context = {run_id: runId, runtime_id: binding.runtime_id, owner_tenant: cell.owner_tenant,
      foreign_tenant: cell.foreign_tenant, nonce: binding.nonce};
    check(safe(context.nonce), 'probe_nonce_missing');
    const specs = probeSpecs(cell, binding);
    const outcomes = [];
    for (const spec of specs) {
      let observation;
      try {
        observation = await call(fixture, spec.kind === 'credential' ? 'credentialProbe'
          : spec.expect === 'allow' ? 'functionalProbe' : 'runtimeProbe', {cell, spec, binding, context});
        validateObservation(observation, spec, binding, context);
        const verdict = evaluateProbe(spec, observation, context);
        outcomes.push({id: spec.id, status: verdict.status, reason: verdict.reason});
      } catch (error) {
        outcomes.push({id: spec.id, status: 'BLOCKED', reason: safe(error?.message) ? error.message : 'probe_unavailable'});
      }
    }
    const statuses = outcomes.map(x => x.status);
    const status = aggregate(statuses);
    const result = {schema: 'sec01-cell-result-v1', id: cell.id, category: cell.category,
      category_id: cell.category_id, state: cell.state, direction: cell.direction, run_id: runId,
      runtime_id: binding.runtime_id, incarnation: binding.incarnation, started, ended: new Date().toISOString(),
      status, attempts: outcomes.length, pass: outcomes.filter(x => x.status === 'PASS').length,
      fail: outcomes.filter(x => x.status === 'FAIL').length, blocked: outcomes.filter(x => x.status === 'BLOCKED').length,
      outcomes, evidence_sha256: digest(outcomes), cleanup: 'PENDING'};
    await record({kind: 'sec01_cell', result});
    return {result, cleanup};
  } catch (error) {
    const result = blocked(cell, safe(error?.message) ? error.message : 'cell_prerequisite_missing', runId);
    result.started = started; result.ended = new Date().toISOString();
    await record({kind: 'sec01_cell', result});
    return {result, cleanup};
  }
}

export function countResults(results) {
  const counts = Object.fromEntries(cells.map(cell => [cell.id, {PASS: 0, FAIL: 0, BLOCKED: 0, NOT_RUN: 0}]));
  for (const result of results) counts[result.id] ??= {PASS: 0, FAIL: 0, BLOCKED: 0, NOT_RUN: 0};
  for (const result of results) counts[result.id][result.status] = (counts[result.id][result.status] ?? 0) + 1;
  return counts;
}

export async function runSec01({fixture, runId = `sec01-${randomUUID()}`, repetitions = 1,
  source, image, record = async () => {}} = {}) {
  check(typeof fixture === 'object' && fixture !== null, 'sec01_fixture_required');
  check(Number.isSafeInteger(repetitions) && repetitions > 0 && repetitions <= 10, 'sec01_repetitions_invalid');
  const results = [], cleanups = [];
  for (let repetition = 1; repetition <= repetitions; repetition += 1) {
    for (const cell of cells) {
      const {result, cleanup} = await runCell(fixture, {...cell, repetition}, runId, record, {source, image});
      results.push({...result, repetition});
      if (typeof cleanup === 'function') cleanups.push({id: cell.id, cleanup});
    }
  }
  const cleanupResults = [];
  for (const item of cleanups.reverse()) {
    try { await item.cleanup(); cleanupResults.push({id: item.id, status: 'PASS'}); }
    catch (error) { cleanupResults.push({id: item.id, status: 'FAIL', reason: 'cleanup_failed'}); }
  }
  const cleanupFailure = cleanupResults.some(x => x.status === 'FAIL');
  if (cleanupFailure) for (const result of results) if (result.status === 'PASS') result.status = 'FAIL';
  const counts = countResults(results);
  const byCategory = Object.fromEntries(Object.keys(CATEGORIES).map(category => {
    const rows = results.filter(result => result.category === category);
    return [category, {cells: rows.length, attempts: rows.reduce((n, x) => n + x.attempts, 0),
      PASS: rows.filter(x => x.status === 'PASS').length, FAIL: rows.filter(x => x.status === 'FAIL').length,
      BLOCKED: rows.filter(x => x.status === 'BLOCKED').length}];
  }));
  return {schema: 'sec01-run-v1', gate: 'M-SEC-01', run_id: runId, runtime_cells: cells.length,
    repetitions, results, counts, by_category: byCategory, cleanup: cleanupResults,
    verdict: results.some(x => x.status === 'FAIL') ? 'FAIL'
      : results.length < cells.length * repetitions || results.some(x => x.status !== 'PASS') ? 'BLOCKED' : 'PASS'};
}

export {validateRuntimeBinding, validateObservation};
