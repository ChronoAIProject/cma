import {readFile, stat} from 'node:fs/promises';
import {resolve, join} from 'node:path';
import {fileURLToPath, pathToFileURL} from 'node:url';

export const ROOT = fileURLToPath(new URL('.', import.meta.url));
export const AUTHORS = ['developer-17', 'developer-06', 'prod-verifier-03',
  'prod-verifier-04', 'developer-15', 'developer-16', 'developer-01', 'developer-09', 'developer-08', 'developer-14'];
const requireValue = (condition, code) => { if (!condition) throw Error(code); };

export function validateCatalog(catalog) {
  requireValue(catalog.schema === 'multi-am-executable-catalog-v1', 'catalog_schema');
  const counts = {user: 120, admin: 76, eg: 79, 'api-supplement': 87};
  requireValue(catalog.rows?.length === 362, 'catalog_row_count');
  requireValue(new Set(catalog.rows.map(r => r.case_id)).size === 362, 'catalog_duplicate_id');
  for (const [family, count] of Object.entries(counts)) {
    requireValue(catalog.rows.filter(r => r.family === family).length === count, 'catalog_family_count');
  }
  for (const row of catalog.rows) {
    requireValue(row.case_id === (row.family === 'api-supplement' ? 'API:' : 'C275:') + row.id,
      'catalog_case_identity');
    requireValue(Array.isArray(row.steps) && row.steps.length > 0
      && JSON.stringify(row.required_steps) === JSON.stringify([
        ...row.steps.map((_, index) => 'step_' + (index + 1)), 'expected_result']), 'catalog_step_identity');
  }
  return catalog;
}

export function validateDefinitions(catalog, assignments, contributions) {
  const rows = new Map(validateCatalog(catalog).rows.map(r => [r.case_id, r]));
  const owners = new Map();
  for (const assignment of assignments) {
    requireValue(AUTHORS.includes(assignment.owner), 'assignment_owner');
    for (const id of assignment.case_ids) {
      requireValue(rows.has(id) && !owners.has(id), 'assignment_overlap_or_unknown');
      owners.set(id, assignment.owner);
    }
  }
  requireValue(owners.size === rows.size, 'assignment_omission');
  const cases = new Map();
  for (const {owner, definitions} of contributions) {
    requireValue(Array.isArray(definitions), 'case_array_required');
    for (const definition of definitions) {
      requireValue(owners.get(definition.id) === owner && !cases.has(definition.id), 'case_owner_or_duplicate');
      requireValue(['agent', 'global', 'browser', 'eg'].includes(definition.scope), 'case_scope');
      requireValue(typeof definition.run === 'function', 'case_run_required');
      requireValue(Array.isArray(definition.steps) && new Set(definition.steps).size === definition.steps.length
        && JSON.stringify([...definition.steps].sort())
          === JSON.stringify([...rows.get(definition.id).required_steps].sort()), 'case_step_coverage');
      requireValue(Array.isArray(definition.fixtures) && definition.fixtures.every(name =>
        typeof name === 'string' && /^[a-z][a-z0-9_-]*$/.test(name)), 'case_fixture_names');
      cases.set(definition.id, {...definition, owner});
    }
  }
  return {catalog, rows, owners, cases};
}

export async function loadRegistry(directory = ROOT) {
  const catalog = JSON.parse(await readFile(join(directory, 'catalog.json'), 'utf8'));
  const assignments = [], contributions = [];
  for (const owner of AUTHORS) {
    assignments.push(JSON.parse(await readFile(join(directory, 'assignments', owner + '.json'), 'utf8')));
    const module = resolve(directory, 'cases', owner, 'index.mjs');
    try { await stat(module); }
    catch (error) { if (error.code === 'ENOENT') continue; throw error; }
    const imported = await import(pathToFileURL(module));
    contributions.push({owner, definitions: imported.cases});
  }
  return validateDefinitions(catalog, assignments, contributions);
}
