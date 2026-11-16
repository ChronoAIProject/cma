import {readFile} from 'node:fs/promises';
import {pathToFileURL} from 'node:url';
import {loadRegistry} from './registry.mjs';
import {validServingReceipt} from './serving.mjs';
import {latencyVerdict} from './latency.mjs';
import {validEgReceipt, platformComplete} from './evidence-scope.mjs';

// Preserve the canonical checklist phase and repetition requirements.
export function phaseRequirements(row) {
  return row.phase_minimum_runs ?? {before_restart: 3, after_restart: 1};
}

export function coverageCounts(registry, results = [], binding = {}) {
  const rows = [];
  const available = new Set(binding.fixtures ?? []);
  for (const row of registry.catalog.rows) {
    const definition = registry.cases.get(row.case_id);
    const missing = definition?.fixtures.filter(name => !available.has(name)) ?? [];
    const exact = results.filter(result => result.id === row.case_id
      && result.source === binding.source && result.image === binding.image
      && typeof binding.campaign_id === 'string' && binding.campaign_id.length > 0
      && result.campaign_id === binding.campaign_id);
    // A namespace such as admin-fixtures says nothing about the 37 authentic
    // recipes below it. Require per-case binding or an actually executed case.
    const ready = binding.executable_case_ids?.includes(row.case_id)
      || exact.some(result => result.status === 'PASS'
        || (result.status === 'FAIL' && (result.requests?.length > 0 || result.eg_requests?.length > 0)));
    const executable = !!definition && missing.length === 0 && !!ready;
    let status = !definition ? 'UNIMPLEMENTED' : !executable ? 'BLOCKED' : 'NOT_RUN';
    if (exact.some(result => result.status === 'FAIL' || (row.target_seconds != null
      && (result.performance === 'FAIL' || latencyVerdict(row, result) === 'FAIL')))) status = 'FAIL';
    else if (exact.some(result => result.status === 'BLOCKED')) status = 'BLOCKED';
    else if (executable && binding.source && binding.image && binding.owner_restart?.verified === true
      && /^[0-9a-f]{64}$/.test(binding.owner_restart?.evidence_sha256 ?? '')) {
      const phases = phaseRequirements(row);
      const complete = Object.entries(phases).every(([phase, minimum]) => {
        const runs = exact.filter(result => result.phase === phase && result.status === 'PASS'
          && (row.target_seconds == null || (result.performance === 'PASS' && latencyVerdict(row, result) === 'PASS'))
          && platformComplete(row.case_id, result)
          && (!result.requires_parent_cleanup || (binding.final_cleanup?.verified === true
            && /^[a-f0-9]{64}$/.test(binding.final_cleanup?.evidence_sha256 ?? '')))
          && (definition.evidence_scope === 'eg_only'
            ? result.eg_requests?.length > 0 && result.eg_requests.every(validEgReceipt)
            : result.requests?.length > 0)
          && result.requests.every(request => ['a', 'b'].includes(request.am)
            && typeof request.request_id === 'string' && request.request_id.trim().length > 0
            && validServingReceipt(request))
          && (!['agent', 'eg'].includes(definition.scope) || definition.evidence_scope === 'eg_only'
            || result.owner_proofs?.some(proof => proof.owner !== proof.ingress))
          && (result.owner_proofs ?? []).every(proof => proof.verdict === 'PASS'
            && (proof.owner !== proof.ingress || proof.kind === 'explicit_ingress'
              || (proof.kind === 'peer_read' && ['GET', 'HEAD'].includes(proof.method)))
            && Number.isSafeInteger(proof.owner_epoch) && proof.owner_epoch > 0)
          && result.steps?.length === row.required_steps.length
          && row.required_steps.every(id => result.steps.some(step => step.id === id
            && step.status === 'PASS' && step.assertions > 0))
          && result.cleanup?.every(cleanup => cleanup.status === 'PASS'));
        return new Set(runs.map(result => result.iteration)).size >= minimum;
      });
      const ingresses = new Set(exact.filter(result => result.status === 'PASS')
        .flatMap(result => definition.evidence_scope === 'eg_only' ? result.eg_requests ?? [] : result.requests ?? [])
        .map(request => definition.evidence_scope === 'eg_only' ? request.replica : request.am));
      if (complete && ingresses.has('a') && ingresses.has('b')) status = 'PASS';
    }
    rows.push({id: row.case_id, family: row.family, registered: !!definition,
      executable, status, missing_fixtures: missing});
  }
  const roles = {};
  for (const family of ['user', 'admin', 'eg', 'api-supplement']) {
    const selected = rows.filter(row => row.family === family);
    roles[family] = {total: selected.length,
      registered: selected.filter(r => r.registered).length,
      executable: selected.filter(r => r.executable).length,
      passing: selected.filter(r => r.status === 'PASS').length,
      failing: selected.filter(r => r.status === 'FAIL').length,
      blocked: selected.filter(r => r.status === 'BLOCKED').length,
      not_run: selected.filter(r => r.status === 'NOT_RUN').length,
      unimplemented: selected.filter(r => r.status === 'UNIMPLEMENTED').length};
  }
  return {schema: 'multi-am-executable-counts-v1', at: new Date().toISOString(),
    source: binding.source ?? null, image: binding.image ?? null,
    fixture_capabilities: [...available].sort(), roles, rows,
    scope: 'C275 plus separate API87; source registration is not runtime acceptance'};
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const packet = process.argv[2] ? JSON.parse(await readFile(process.argv[2], 'utf8')) : {};
  const registry = await loadRegistry();
  console.log(JSON.stringify(coverageCounts(registry, packet.results ?? [], packet.binding ?? {}), null, 2));
}
