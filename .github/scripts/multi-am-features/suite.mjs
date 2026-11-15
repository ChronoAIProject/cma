import {mkdir, open, readFile, writeFile} from 'node:fs/promises';
import {join, dirname} from 'node:path';
import {fileURLToPath} from 'node:url';
import {API, normalizeConfig, check} from './cma/core.mjs';
import {runJourney} from './cma/journey.mjs';
import {ExtendedAdminMatrix} from './features/admin_extended.mjs';
import {ADMIN_PERMISSIONS} from './features/admin_matrix.mjs';
import {userCases} from './features/user.mjs';
import {coverage, loadInventory} from './coverage.mjs';

const root = dirname(fileURLToPath(import.meta.url));
const config = JSON.parse(await readFile(process.argv[2], 'utf8'));
const inventory = await loadInventory(join(root, 'inventory'));
const mappings = JSON.parse(await readFile(join(root, 'case-mapping.json'), 'utf8'));
await mkdir(config.matrix_output, {recursive: true, mode: 0o700});
const rows = [], observations = [];
const journal = await open(join(config.matrix_output, 'cases.private.jsonl'), 'ax', 0o600);
async function record(row) {
  rows.push(row); await journal.write(JSON.stringify(row) + '\n'); await journal.sync();
}
const normal = normalizeConfig(config);
const user = new API(normal);
const admin = new API({...normal, bearer: config.admin_bearer});
const fixture = {scope: 'LOCAL_DISPOSABLE_TWO_AM', source: config.expected_api_source,
  manifest: config.expected_api_image.split('@')[1], database_disposable: true, single_matrix_writer: true,
  am_origins: normal.ams, run_id: config.run_id, admin_permissions: ADMIN_PERMISSIONS, ordinary_user_has_admin: false};
const matrix = new ExtendedAdminMatrix({fixture,
  request: (role, ...args) => (role === 'admin' ? admin : user).request(...args),
  record: row => record({case: 'admin.' + row.feature, verdict: row.verdict,
    iteration: row.iteration, elapsed_ms: row.elapsed_ms, error_code: row.error_code,
    ams: [...new Set(row.requests.map(x => x.am))],
    routing: 'global_admin_has_no_agent_owner', requests: row.requests})});
const result = {schema: 'multi-am-features-v1', verdict: 'RUNNING', source: config.expected_api_source,
  production_acceptance: false, image: config.expected_api_image, started_at: new Date().toISOString()};
try {
  result.journey = await runJourney(config, {featureHook: async context => {
    // The same admin groups run before and after actual owner loss. Their public
    // mutations use CAS, peer readback, stale-revision refusal and restoration.
    Object.assign(normal.ams, context.config.ams);
    await matrix.run({iterations: 3, groups: ['reads', 'denials', 'settings', 'triggers', 'admission', 'usage']});
    await matrix.runExtended({iterations: 3, groups: ['replace', 'inactivity']});
    const previousRecorder = context.api.router.record;
    context.api.router.record = async proof => { observations.push(proof); await previousRecorder(proof); };
    try {
      for (let iteration = 1; iteration <= 3; iteration++) await userCases({...context,
        runCase: async (name, operation) => {
          const start = performance.now(), first = observations.length;
          const row = {case: name, phase: context.phase, iteration, verdict: 'RUNNING'};
          try { await operation(); row.verdict = 'PASS'; }
          catch (error) { row.verdict = 'FAIL'; throw error; }
          finally {
            const proofs = observations.slice(first);
            row.non_owner = proofs.length > 0 && proofs.every(p => p.verdict === 'PASS' && p.owner !== p.ingress);
            row.elapsed_ms = performance.now() - start; await record(row);
          }
        }});
    } finally { context.api.router.record = previousRecorder; }
    return {assertions: rows.length, non_owner_requests: observations.length};
  }});
  check(result.journey.verdict === 'LOCAL_TWO_AM_JOURNEY_PASS', 'two_am_journey_failed');
  result.verdict = 'EXECUTABLE_CASES_PASS';
} catch (error) {
  result.verdict = 'FAIL';
  result.failure = /^[a-zA-Z0-9_]+$/.test(error.message ?? '') ? error.message : 'feature_contract_or_transport_error';
} finally {
  await admin.close(); await user.close(); await journal.close();
  result.coverage = coverage(inventory, mappings, rows);
  if (result.verdict !== 'FAIL') result.verdict = result.coverage.verdict;
  result.finished_at = new Date().toISOString();
  await writeFile(join(config.matrix_output, 'result.private.json'), JSON.stringify(result, null, 2), {mode: 0o600});
  // This allowlist is the only suite content eligible for CI artifact upload.
  await writeFile(join(config.summary_dir, 'coverage.json'), JSON.stringify({
    ...result.coverage, suite_verdict: result.verdict, source: result.source,
    case_count: rows.length, non_owner_request_count: observations.length,
    owner_restart_pass: result.journey?.steps.some(s => s.name === 'actual_owner_kill_and_recovery' && s.verdict === 'PASS') === true,
  }, null, 2), {mode: 0o600});
}
process.exitCode = result.verdict === 'PASS' ? 0 : 1;
