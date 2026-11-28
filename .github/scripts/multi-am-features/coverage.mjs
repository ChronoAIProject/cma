import {readFile, readdir} from 'node:fs/promises';
import {join} from 'node:path';
import {createHash} from 'node:crypto';

const requireValue = (value, message) => { if (!value) throw new Error(message); };

export async function loadInventory(directory) {
  const features = [];
  const manifest = JSON.parse(await readFile(join(directory, 'manifest.json'), 'utf8'));
  requireValue(manifest.schema === 'multi-am-inventory-v2', 'inventory_schema');
  requireValue(Object.keys(manifest.counts).sort().join(',') === 'admin,eg,user', 'inventory_groups');
  for (const role of ['user', 'admin', 'eg']) {
    const files = (await readdir(join(directory, role))).filter(x => x.endsWith('.json')).sort();
    requireValue(files.length > 0, 'inventory_role_missing_' + role);
    const rows = [];
    for (const file of files) rows.push(...JSON.parse(await readFile(join(directory, role, file), 'utf8')));
    requireValue(rows.length === manifest.counts[role], 'inventory_count_drift_' + role);
    requireValue(rows.every(row => row.role === role), 'inventory_role_mismatch');
    const ids = rows.map(row => row.id).sort();
    requireValue(createHash('sha256').update(ids.join('\n')).digest('hex') === manifest.id_set_sha256[role],
      'inventory_identity_drift_' + role);
    features.push(...rows);
  }
  requireValue(features.every(row => typeof row.id === 'string' && row.id.length > 0)
    && new Set(features.map(x => x.id)).size === features.length, 'inventory_duplicate_or_missing_id');
  return features;
}

export function coverage(features, mappings, results) {
  const known = new Set(features.map(x => x.id));
  requireValue(Object.keys(mappings).every(id => known.has(id)), 'mapping_unknown_feature');
  requireValue(results.every(x => typeof x.case === 'string'), 'case_identity_missing');
  const rows = features.map(feature => {
    const mapping = mappings[feature.id];
    const required = mapping?.cases ?? [];
    const blocked = mapping?.blocked;
    if (blocked || !required.length) return {id: feature.id, role: feature.role, verdict: 'BLOCKED',
      reason: blocked ?? 'executable_assertion_missing'};
    const observations = required.map(name => results.filter(x => x.case === name));
    if (observations.some(group => !group.length)) return {id: feature.id, role: feature.role,
      verdict: 'NOT_RUN', cases: required};
    const flat = observations.flat();
    const repetitions = group => {
      const phases = mapping.phases ?? [undefined];
      return phases.every(phase => {
        const runs = group.filter(row => phase === undefined || row.phase === phase);
        const iterations = new Set(runs.map(row => row.iteration).filter(Number.isSafeInteger));
        const complete = [...iterations].filter(iteration => mapping.routing !== 'both'
          || ['a', 'b'].every(am => runs.some(row => row.iteration === iteration && row.ams?.includes(am))));
        return complete.length
          >= (mapping.minimum_runs ?? 1);
      });
    };
    const pass = flat.every(x => x.verdict === 'PASS')
      && observations.every(repetitions)
      && (mapping.routing !== 'non_owner' || flat.every(x => x.non_owner === true))
      && (mapping.browser !== true || flat.every(x => x.real_browser === true));
    return {id: feature.id, role: feature.role, verdict: pass ? 'PASS' : 'FAIL', cases: required};
  });
  const counts = Object.fromEntries(['PASS', 'FAIL', 'BLOCKED', 'NOT_RUN']
    .map(verdict => [verdict, rows.filter(x => x.verdict === verdict).length]));
  return {schema: 'multi-am-coverage-v1', verdict: counts.PASS === rows.length && rows.length > 0 ? 'PASS' : 'FAIL',
    total: rows.length, counts, features: rows};
}
