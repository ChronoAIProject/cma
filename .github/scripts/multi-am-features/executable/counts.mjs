import {readFile} from 'node:fs/promises';
import {pathToFileURL} from 'node:url';
import {loadRegistry} from './registry.mjs';

export function coverageCounts(registry, results = [], binding = {}) {
  const rows = [];
  const available = new Set(binding.fixtures ?? []);
  for (const row of registry.catalog.rows) {
    const definition = registry.cases.get(row.case_id);
    const missing = definition?.fixtures.filter(name => !available.has(name)) ?? [];
    const executable = !!definition && missing.length === 0;
    const exact = results.filter(result => result.id === row.case_id
      && result.source === binding.source && result.image === binding.image);
    let status = !definition ? 'UNIMPLEMENTED' : missing.length ? 'BLOCKED' : 'NOT_RUN';
    if (exact.some(result => result.status === 'FAIL')) status = 'FAIL';
    else if (exact.some(result => result.status === 'BLOCKED')) status = 'BLOCKED';
    else if (executable && binding.source && binding.image && binding.owner_restart?.verified === true
      && /^[0-9a-f]{64}$/.test(binding.owner_restart?.evidence_sha256 ?? '')) {
      const phases = row.phase_minimum_runs ?? {before_restart: 3, after_restart: 1};
      const complete = Object.entries(phases).every(([phase, minimum]) => {
        const runs = exact.filter(result => result.phase === phase && result.status === 'PASS'
          && result.requests?.length > 0
          && result.requests.every(request => ['a', 'b'].includes(request.am)
            && typeof request.request_id === 'string' && request.request_id.trim().length > 0)
          && (definition.scope !== 'agent' || result.owner_proofs?.length > 0)
          && (result.owner_proofs ?? []).every(proof => proof.verdict === 'PASS'
            && proof.owner !== proof.ingress && Number.isSafeInteger(proof.owner_epoch) && proof.owner_epoch > 0)
          && result.steps?.length === row.required_steps.length
          && row.required_steps.every(id => result.steps.some(step => step.id === id
            && step.status === 'PASS' && step.assertions > 0))
          && result.cleanup?.every(cleanup => cleanup.status === 'PASS'));
        return new Set(runs.map(result => result.iteration)).size >= minimum;
      });
      const ingresses = new Set(exact.filter(result => result.status === 'PASS')
        .flatMap(result => result.requests ?? []).map(request => request.am));
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
