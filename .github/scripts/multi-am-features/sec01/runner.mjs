import {randomUUID} from 'node:crypto';
import {cells, CATEGORIES} from './cells.mjs';
import {aggregate, evaluateProbe, digest} from './evaluate.mjs';
import {check, identity, sourceSha, imageDigest, errorCode, validateRuntimeBinding,
  validateObservation, retainedObservation} from './binding.mjs';
export {validateRuntimeBinding, validateObservation} from './binding.mjs';

async function call(fixture, name, args) {
  if (typeof fixture?.[name] !== 'function') throw Error('fixture_hook_unavailable');
  return fixture[name](args);
}

export function probeSpecs(cell, binding) {
  const base = {cell_id: cell.id, state: cell.state, direction: cell.direction,
    owner_tenant: cell.owner_tenant, foreign_tenant: cell.foreign_tenant};
  const category = CATEGORIES[cell.category];
  const fileProbes = ['foreign_home', 'foreign_workspace', 'foreign_volume', 'traversal', 'symlink',
    'proc_root_alias', 'retired_recycled', 'host_mount', 'projected_token', 'host_mount_token'];
  const httpProbes = ['redirect_destination', 'proxy_direct', 'metadata_id'];
  const specs = category.probes.map(probe => ({...base, id: `${cell.id}:${probe}`,
    kind: cell.category === 'credential' ? 'credential' : fileProbes.includes(probe) ? 'file'
      : httpProbes.includes(probe) ? 'http' : 'tcp',
    scenario: probe, expect: 'deny', owner_control: binding.targets?.[probe] ?? null,
    denial_code: binding.targets?.[probe]?.denial_code,
  }));
  for (const positive of category.positive) specs.push({
    ...base, id: `${cell.id}:positive:${positive}`,
    kind: cell.category === 'credential' ? 'credential' : cell.category === 'file' ? 'file' : 'http',
    scenario: positive, operation: positive, expect: 'allow', expected_sha256: binding.positives?.[positive]?.sha256,
    expected_bytes: binding.positives?.[positive]?.bytes,
  });
  return specs;
}

async function runCell(fixture, cell, runId, retain, candidate, signal) {
  const result = {schema: 'sec01-cell-result-v1', id: cell.id, category: cell.category,
    state: cell.state, direction: cell.direction, repetition: cell.repetition, run_id: runId,
    source: candidate.source, image: candidate.image, started: new Date().toISOString(),
    status: 'BLOCKED', attempts: 0, pass: 0, fail: 0, blocked: 0, outcomes: [], cleanup: 'NOT_ACQUIRED'};
  let binding, cleanup;
  try {
    check(!signal?.aborted, 'campaign_cancelled');
    // The parent must register any uncertain setup intent in its cleanup ledger
    // before mutation, including a prepareCell failure that returns no handle.
    binding = await call(fixture, 'prepareCell', {cell, run_id: runId, ...candidate, signal});
    cleanup = binding?.cleanup;
    result.cleanup = typeof cleanup === 'function' ? 'PENDING' : 'MISSING';
    validateRuntimeBinding(cell, binding, runId, candidate);
    check(typeof cleanup === 'function', 'cleanup_hook_missing');
    result.binding_sha256 = binding.evidence_sha256;
    result.runtime_id = binding.runtime_id;
    result.incarnation = binding.incarnation;
    for (const am of ['a', 'b']) for (const spec of probeSpecs(cell, binding)) {
      const context = {run_id: runId, runtime_id: binding.runtime_id, owner_tenant: cell.owner_tenant,
        foreign_tenant: cell.foreign_tenant, nonce: binding.nonce, am};
      let observation, verdict = {status: 'BLOCKED', reason: 'probe_not_run'}, evidenceError;
      try {
        check(!signal?.aborted, 'campaign_cancelled');
        observation = await call(fixture, spec.kind === 'credential' ? 'credentialProbe'
          : ['file', 'tcp'].includes(spec.kind) ? 'runtimeProbe' : 'functionalProbe',
        {cell, spec, binding, context, signal});
        // Preserve demonstrated forbidden success even if attribution is incomplete.
        verdict = evaluateProbe(spec, observation, context);
        validateObservation(observation, spec, binding, context);
      } catch (error) {
        evidenceError = errorCode(error);
        if (verdict.status !== 'FAIL') verdict = {status: 'BLOCKED', reason: evidenceError};
      }
      if (observation !== undefined) {
        try {
          const captured = retainedObservation(observation);
          await retain({kind: 'sec01_probe_observation', case_id: cell.id, repetition: cell.repetition,
            probe_id: spec.id, am, observation: captured});
        } catch { verdict = {status: 'FAIL', reason: 'private_evidence_write_failed'}; }
      }
      result.outcomes.push({id: spec.id, am, ...verdict, ...(evidenceError ? {evidence_error: evidenceError} : {})});
    }
    result.status = aggregate(result.outcomes.map(x => x.status));
  } catch (error) { result.reason = errorCode(error); }
  finally {
    if (typeof cleanup === 'function') {
      try { await cleanup(); result.cleanup = 'PASS'; }
      catch { result.cleanup = 'FAIL'; result.status = 'FAIL'; result.cleanup_error = 'cleanup_failed'; }
    }
    result.ended = new Date().toISOString();
    result.attempts = result.outcomes.length;
    for (const [key, status] of [['pass', 'PASS'], ['fail', 'FAIL'], ['blocked', 'BLOCKED']]) {
      result[key] = result.outcomes.filter(x => x.status === status).length;
    }
    result.evidence_sha256 = digest(result.outcomes);
    try { await retain({kind: 'sec01_cell_final', result}); }
    catch { result.status = 'FAIL'; result.evidence_error = 'private_evidence_write_failed'; }
  }
  return result;
}

export function countResults(results) {
  const counts = Object.fromEntries(cells.map(cell => [cell.id, {PASS: 0, FAIL: 0, BLOCKED: 0, NOT_RUN: 0}]));
  for (const result of results) {
    check(counts[result.id] && ['PASS', 'FAIL', 'BLOCKED', 'NOT_RUN'].includes(result.status), 'unknown_result');
    counts[result.id][result.status] += 1;
  }
  return counts;
}

export async function runSec01({fixture, runId = `sec01-${randomUUID()}`, repetitions = 1,
  source, image, runtimeSource, runtimeImage, lease, signal, record} = {}) {
  check(fixture && typeof fixture === 'object', 'sec01_fixture_required');
  check(typeof record === 'function', 'private_evidence_recorder_required');
  check(lease?.scope === 'LOCAL_DISPOSABLE_TWO_AM' && lease.database_disposable === true
    && lease.single_matrix_writer === true && lease.restart_authority === 'PARENT_ONLY', 'parent_lease_required');
  check(identity(runId) && sourceSha(source) && imageDigest(image)
    && sourceSha(runtimeSource) && imageDigest(runtimeImage), 'candidate_binding_required');
  check(Number.isSafeInteger(repetitions) && repetitions > 0 && repetitions <= 10, 'sec01_repetitions_invalid');
  const results = [];
  let recordFailure = false;
  const retain = async event => {
    try { await record(event); }
    catch (error) { recordFailure = true; throw error; }
  };
  for (let repetition = 1; repetition <= repetitions; repetition += 1) for (const cell of cells) {
    results.push(await runCell(fixture, {...cell, repetition}, runId, retain,
      {source, image, runtimeSource, runtimeImage}, signal));
  }
  const byCategory = Object.fromEntries(Object.keys(CATEGORIES).map(category => {
    const rows = results.filter(result => result.category === category);
    return [category, {cells: cells.filter(x => x.category === category).length, cell_attempts: rows.length,
      probe_attempts: rows.reduce((n, x) => n + x.attempts, 0),
      PASS: rows.filter(x => x.status === 'PASS').length, FAIL: rows.filter(x => x.status === 'FAIL').length,
      BLOCKED: rows.filter(x => x.status === 'BLOCKED').length}];
  }));
  const packet = {schema: 'sec01-run-v1', gate: 'M-SEC-01', scope: 'LOCAL_RUNTIME_REHEARSAL',
    run_id: runId, source, image, runtime_source: runtimeSource, runtime_image: runtimeImage,
    runtime_cells: cells.length, repetitions, results, counts: countResults(results), by_category: byCategory,
    cleanup: results.filter(x => x.cleanup !== 'NOT_ACQUIRED').map(x => ({id: x.id, repetition: x.repetition, status: x.cleanup})),
    verdict: recordFailure ? 'FAIL' : aggregate(results.map(x => x.status))};
  try { await retain({kind: 'sec01_run_final', packet}); }
  catch { packet.verdict = 'FAIL'; packet.evidence_error = 'private_evidence_write_failed'; }
  return packet;
}
