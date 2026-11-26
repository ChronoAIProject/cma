import {mkdir, open, readFile, writeFile} from 'node:fs/promises';
import {join, dirname} from 'node:path';
import {fileURLToPath} from 'node:url';
import {API, normalizeConfig, check} from './cma/core.mjs';
import {runJourney} from './cma/journey.mjs';
import {ExtendedAdminMatrix} from './features/admin_extended.mjs';
import {ADMIN_PERMISSIONS} from './features/admin_matrix.mjs';
import {userCases} from './features/user.mjs';
import {documentCases} from './features/documents.mjs';
import {configurationCases, heldConfigurationCases, assertWriterTitle,
  verifyLifecycleReplay} from './features/agent_configuration.mjs';
import {coverage, loadInventory} from './coverage.mjs';
import {readConfig} from './config.mjs';
import {retainFailure} from './failure.mjs';
import {NonOwnerRouter} from './routing.mjs';
import {writerCoverage} from './writer_coverage.mjs';
import {queuedCommandCase} from './features/queued_commands.mjs';
import {configurationDenials} from './features/configuration_denials.mjs';
import {installSkillCase} from './features/skills.mjs';
import {shareAgentArtifact} from './features/sharing.mjs';
import {organizationShare} from './features/organization_share.mjs';
import {accessRequest} from './features/access_request.mjs';
import {triggerAlwaysOn} from './features/trigger_always_on.mjs';

const root = dirname(fileURLToPath(import.meta.url));
// Synthetic credentials travel over the parent's pipe, never a config file or argv.
const config = await readConfig(process.stdin);
const inventory = await loadInventory(join(root, 'inventory'));
const mappings = JSON.parse(await readFile(join(root, 'case-mapping.json'), 'utf8'));
const writerRegistry = JSON.parse(await readFile(join(root, 'writer-regressions.json'), 'utf8'));
await mkdir(config.matrix_output, {recursive: true, mode: 0o700});
const rows = [], observations = [], adminOwnerProofs = [], apiObservations = [], savedDocuments = [];
let phase;
const journal = await open(join(config.matrix_output, 'cases.private.jsonl'), 'ax', 0o600);
async function record(row) {
  rows.push(row); await journal.write(JSON.stringify(row) + '\n'); await journal.sync();
}
const normal = normalizeConfig(config);
check(typeof config.outsider_bearer === 'string' && config.outsider_bearer.length > 0
  && config.outsider_bearer !== normal.bearer && config.outsider_bearer !== config.admin_bearer,
'independent_outsider_required');
const user = new API(normal, observation => apiObservations.push(observation));
const admin = new API({...normal, bearer: config.admin_bearer}, observation => apiObservations.push(observation));
const outsider = new API({...normal, bearer: config.outsider_bearer}, observation => apiObservations.push(observation));
async function documentCase(iteration, name, operation) {
  const start = performance.now(), first = apiObservations.length;
  const row = {case: name, phase, iteration, verdict: 'RUNNING', routing: 'shared_document_has_no_agent_owner'};
  try { const value = await operation(); row.verdict = 'PASS'; return value; }
  catch (error) { row.verdict = 'FAIL'; throw error; }
  finally {
    row.ams = [...new Set(apiObservations.slice(first).map(x => x.am))];
    row.elapsed_ms = performance.now() - start;
    await record(row);
  }
}
const fixture = {scope: 'LOCAL_DISPOSABLE_TWO_AM', source: config.expected_api_source,
  manifest: config.expected_api_image.split('@')[1], database_disposable: true, single_matrix_writer: true,
  am_origins: normal.ams, run_id: config.run_id, admin_permissions: ADMIN_PERMISSIONS, ordinary_user_has_admin: false};
const matrix = new ExtendedAdminMatrix({fixture,
  onFailure: (stage, error, row) => retainFailure(config.matrix_output, stage, error,
    {case: 'admin.' + row?.feature, phase, iteration: row?.iteration}),
  request: (role, ...args) => (role === 'admin' ? admin : user).request(...args),
  record: row => {
    const scoped = row.feature === 'sandbox_always_on_cross_am';
    const ids = row.requests.map(request => request.request_id).filter(Boolean);
    const proofs = adminOwnerProofs.filter(proof => ids.includes(proof.request_id));
    return record({case: 'admin.' + row.feature, verdict: row.verdict,
    phase, iteration: row.iteration, elapsed_ms: row.elapsed_ms, error_code: row.error_code,
    primary_error_code: row.primary_error_code, restoration_error_code: row.restoration_error_code,
    failures: row.failures,
    ams: [...new Set(row.requests.map(x => x.am))],
    non_owner: scoped && ids.length === row.requests.length && proofs.length === ids.length
      && proofs.every(proof => proof.verdict === 'PASS' && proof.owner !== proof.ingress),
    routing: scoped ? 'agent_bound_sandbox_non_owner' : 'global_admin_has_no_agent_owner',
    requests: row.requests, owner_proofs: proofs});
  }});
const result = {schema: 'multi-am-features-v1', verdict: 'RUNNING', source: config.expected_api_source,
  production_acceptance: false, image: config.expected_api_image, started_at: new Date().toISOString()};
try {
  result.journey = await runJourney(config, {triggerHook: context => {
    Object.assign(normal.ams, context.config.ams);
    return triggerAlwaysOn({...context, user, admin, outsider});
  }, queuedTurnHook: async context => {
    const previousRecorder = context.api.router.record;
    context.api.router.record = async proof => { observations.push(proof); await previousRecorder(proof); };
    try {
      for (let iteration = 1; iteration <= 3; iteration++) {
        const first = observations.length;
        const row = {case: 'writer.queued_edit_cancel', phase: context.phase, iteration,
          verdict: 'RUNNING', states: []};
        try {
          await queuedCommandCase({...context, recordState: async (stage, state) => row.states.push({stage, state})});
          row.verdict = 'PASS';
        } catch (error) {
          row.verdict = 'FAIL';
          try { row.failures = [await retainFailure(config.matrix_output, 'primary', error,
            {case: row.case, phase: row.phase, iteration})]; }
          catch { row.evidence_failure = true; }
          throw error;
        } finally {
          row.owner_proofs = observations.slice(first);
          row.non_owner = row.owner_proofs.length > 0
            && row.owner_proofs.every(proof => proof.verdict === 'PASS' && proof.owner !== proof.ingress);
          await record(row);
        }
      }
    } finally { context.api.router.record = previousRecorder; }
  }, heldTurnHook: async context => {
    const previousRecorder = context.api.router.record;
    context.api.router.record = async proof => { observations.push(proof); await previousRecorder(proof); };
    try {
      for (let iteration = 1; iteration <= 3; iteration++) {
        let activeRow;
        await heldConfigurationCases({...context,
          recordState: async (stage, state) => { (activeRow.states ??= []).push({stage, state}); },
          runCase: async (name, operation) => {
          const start = performance.now(), first = observations.length;
          const row = {case: name, phase: context.phase, iteration, verdict: 'RUNNING'};
          activeRow = row;
          try { await operation(); row.verdict = 'PASS'; }
          catch (error) {
            row.verdict = 'FAIL';
            try { row.failures = [await retainFailure(config.matrix_output, 'primary', error,
              {case: name, phase: context.phase, iteration})]; }
            catch { row.evidence_failure = true; }
            throw error;
          } finally {
            row.owner_proofs = observations.slice(first);
            row.non_owner = row.owner_proofs.length > 0
              && row.owner_proofs.every(p => p.verdict === 'PASS' && p.owner !== p.ingress);
            row.elapsed_ms = performance.now() - start; await record(row);
          }
        }});
      }
    } finally { context.api.router.record = previousRecorder; }
  }, featureHook: async context => {
    phase = context.phase;
    // The same admin groups run before and after actual owner loss. Their public
    // mutations use CAS, peer readback, stale-revision refusal and restoration.
    Object.assign(normal.ams, context.config.ams);
    admin.router = new NonOwnerRouter({adapter: context.adapter,
      record: proof => { adminOwnerProofs.push(proof); observations.push(proof); }});
    admin.bindSandbox(context.record.sandboxId, context.record.agentId);
    outsider.router = new NonOwnerRouter({adapter: context.adapter, record: proof => observations.push(proof)});
    outsider.routedAgents.add(context.record.agentId);
    if (phase === 'after_restart') {
      const first = observations.length;
      const previousRecorder = context.api.router.record;
      context.api.router.record = async proof => { observations.push(proof); await previousRecorder(proof); };
      const row = {case: 'writer.held_title_survives_restart', phase, iteration: 1, verdict: 'RUNNING'};
      try { row.final_state = await assertWriterTitle(context.api, context.adapter, context.record); row.verdict = 'PASS'; }
      catch (error) { row.verdict = 'FAIL'; throw error; }
      finally {
        context.api.router.record = previousRecorder;
        row.owner_proofs = observations.slice(first);
        row.non_owner = row.owner_proofs.length > 0
          && row.owner_proofs.every(p => p.verdict === 'PASS' && p.owner !== p.ingress);
        await record(row);
      }
    }
    if (phase === 'after_restart') for (const [index, document] of savedDocuments.entries()) {
      await documentCase(index + 1, 'user.documents_survive_restart', async () => {
        for (const am of ['a', 'b']) {
          const current = (await user.request(am, 'GET', '/api/v1/documents/' + encodeURIComponent(document.id))).body;
          check(current.id === document.id && current.revision === document.revision
            && current.sha256 === document.sha256, 'document_changed_across_owner_restart');
          const source = (await user.request(am, 'GET', '/api/v1/documents/' + encodeURIComponent(document.id) + '/source')).body;
          check(source.id === document.id && source.revision === document.source_revision,
            'document_source_missing_after_restart');
        }
      });
    }
    for (let iteration = 1; iteration <= 3; iteration++) {
      const document = await documentCases({owner: user, recipient: admin,
        ownerId: config.owner_id, recipientId: 'native-am-fixture-admin',
        runCase: (name, operation) => documentCase(iteration, name, operation)});
      if (phase === 'before_restart') savedDocuments.push(document);
    }
    await matrix.run({iterations: 3, groups: ['reads', 'denials', 'settings', 'triggers', 'admission', 'usage']});
    await matrix.runExtended({iterations: 3, groups: ['replace', 'inactivity']});
    const previousRecorder = context.api.router.record;
    context.api.router.record = async proof => { observations.push(proof); await previousRecorder(proof); };
    try {
      for (let iteration = 1; iteration <= 3; iteration++) {
        let activeRow;
        const caseContext = {...context,
        recordState: async (stage, state) => { (activeRow.states ??= []).push({stage, state}); },
        recordFailure: async (stage, error) => {
          const receipt = await retainFailure(config.matrix_output, stage, error,
            {case: activeRow?.case, phase: context.phase, iteration});
          if (activeRow) (activeRow.failures ??= []).push(receipt);
        },
        runCase: async (name, operation) => {
          const start = performance.now(), first = observations.length;
          const firstRequest = apiObservations.length;
          const row = {case: name, phase: context.phase, iteration, verdict: 'RUNNING'};
          activeRow = row;
          try { await operation(); row.verdict = 'PASS'; }
          catch (error) { row.verdict = 'FAIL'; throw error; }
          finally {
            const proofs = observations.slice(first);
            row.non_owner = proofs.length > 0 && proofs.every(p => p.verdict === 'PASS' && p.owner !== p.ingress);
            row.owner_proofs = proofs;
            row.ams = [...new Set(apiObservations.slice(firstRequest).map(request => request.am))];
            row.elapsed_ms = performance.now() - start; await record(row);
          }
        }};
        if (iteration === 1) {
          await caseContext.runCase('writer.skill_installs_once', () => installSkillCase(caseContext));
          await caseContext.runCase('writer.share_and_artifact_receipts',
            () => shareAgentArtifact({...caseContext, user, outsider}));
          await caseContext.runCase('writer.organization_share_membership',
            () => organizationShare({...caseContext, user, outsider}));
          await caseContext.runCase('writer.agent_access_request_denial',
            () => accessRequest({...caseContext, user, outsider}));
        }
        await userCases(caseContext);
        await configurationDenials({...caseContext, outsider});
        await configurationCases(caseContext);
        await configurationCases({...caseContext, api: admin,
          runCase: (name, operation) => caseContext.runCase(name.replace('user.', 'admin.'), operation)});
        await matrix.sandboxAlwaysOn(iteration, {
          created_by_run: config.run_id, sandbox_id: context.record.sandboxId,
          verifyAdmission: async stage => {
            for (const am of ['a', 'b']) {
              const detail = (await context.api.rawRequest(am, 'GET',
                '/api/v1/agents/' + context.record.agentId)).body;
              check(detail.id === context.record.agentId && detail.sandbox?.id === context.record.sandboxId
                && detail.status === 'ready', 'always_on_public_agent_unavailable_' + stage);
            }
            // Replay only acknowledged creation custody; altered payload must
            // refuse without replacing the original receipt or creating again.
            await verifyLifecycleReplay(context.api, context.record);
          },
        });
      }
    } finally { context.api.router.record = previousRecorder; }
    return {assertions: rows.length, non_owner_requests: observations.length};
  }});
  check(result.journey.verdict === 'LOCAL_TWO_AM_JOURNEY_PASS', 'two_am_journey_failed');
  result.verdict = 'EXECUTABLE_CASES_PASS';
} catch (error) {
  result.verdict = 'FAIL';
  result.failure = /^[a-zA-Z0-9_]+$/.test(error.message ?? '') ? error.message : 'feature_contract_or_transport_error';
} finally {
  await admin.close(); await user.close(); await outsider.close(); await journal.close();
  result.coverage = coverage(inventory, mappings, rows);
  result.writer_coverage = writerCoverage(writerRegistry, rows, result.journey, 'features');
  if (result.verdict !== 'FAIL') result.verdict = result.coverage.verdict === 'PASS'
    && result.writer_coverage.verdict === 'PASS' ? 'PASS' : 'FAIL';
  result.finished_at = new Date().toISOString();
  await writeFile(join(config.matrix_output, 'result.private.json'), JSON.stringify(result, null, 2), {mode: 0o600});
  const revival = result.journey?.steps.find(s => s.name === 'ownerless_pause_revive_open' && s.verdict === 'PASS');
  // This allowlist is the only suite content eligible for CI artifact upload.
  await writeFile(join(config.summary_dir, 'coverage.json'), JSON.stringify({
    ...result.coverage, suite_verdict: result.verdict, source: result.source,
    writer_coverage: result.writer_coverage,
    case_count: rows.length, non_owner_request_count: observations.length,
    owner_restart_pass: result.journey?.steps.some(s => s.name === 'actual_owner_kill_and_recovery' && s.verdict === 'PASS') === true,
    ownerless_revival: revival ? {ingress: revival.ingress, replay_ingress: revival.replay_ingress,
      prior_owner_epoch: revival.prior_owner_epoch, owner_epoch: revival.owner_epoch,
      durable_admissions: revival.durable_admissions, durable_completions: revival.durable_completions,
      retained_owner_restarted: revival.retained_owner_restarted, request_sha256: revival.request_sha256,
      replay_request_sha256: revival.replay_request_sha256} : null,
    autoscaler_ready_manager_counts: result.journey?.steps.find(s =>
      s.name === 'ready_autoscaler_two_three_two' && s.verdict === 'PASS')?.ready_managers ?? [],
    never_ready_manager_samples: result.journey?.steps.find(s =>
      s.name === 'never_ready_third_manager' && s.verdict === 'PASS')?.never_ready_samples ?? 0,
  }, null, 2), {mode: 0o600});
}
process.exitCode = result.verdict === 'PASS' ? 0 : 1;
