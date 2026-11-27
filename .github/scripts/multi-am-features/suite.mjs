import {mkdir, open, readFile, writeFile} from 'node:fs/promises';
import {join, dirname} from 'node:path';
import {fileURLToPath} from 'node:url';
import {API, normalizeConfig, check} from './cma/core.mjs';
import {runJourney} from './cma/journey.mjs';
import {requestPacer} from './pacing.mjs';
import {ExtendedAdminMatrix} from './features/admin_extended.mjs';
import {ADMIN_PERMISSIONS, verifyRuntimeInventory} from './features/admin_matrix.mjs';
import {userCases} from './features/user.mjs';
import {browserFileRoundtrip} from './features/files_browser.mjs';
import {browserChangedFilePreview, withChangedFile} from './features/files_preview.mjs';
import {documentCases} from './features/documents.mjs';
import {createDocumentAfterLostReply} from './features/document_creation_browser.mjs';
import {documentEditAndConflict} from './features/document_edit_browser.mjs';
import {documentBrowse} from './features/document_browse_browser.mjs';
import {documentPeople} from './features/document_people_browser.mjs';
import {documentCommentBrowser} from './features/document_comment_browser.mjs';
import {busyConfigurationCase, competingSystemPrompts, configurationCases, heldConfigurationCases, assertWriterTitle,
  verifyLifecycleReplay} from './features/agent_configuration.mjs';
import {coverage, loadInventory} from './coverage.mjs';
import {readConfig} from './config.mjs';
import {journeyFailureSummary, rateLimitEvidence, retainFailure, withRestoration} from './failure.mjs';
import {openPreparedBrowser} from './prepared_browser.mjs';
import {relinquishAndReplayAlwaysOn} from './features/sandbox_always_on_browser.mjs';
import {NonOwnerRouter} from './routing.mjs';
import {writerCoverage} from './writer_coverage.mjs';
import {queuedCommandCase} from './features/queued_commands.mjs';
import {configurationDenials} from './features/configuration_denials.mjs';
import {installSkillCase} from './features/skills.mjs';
import {artifactCommittedWithoutAcknowledgement, artifactUnknownPublication, revokeWhileRedemptionRetries,
  shareAgentArtifact} from './features/sharing.mjs';
import {organizationShare} from './features/organization_share.mjs';
import {accessRequest} from './features/access_request.mjs';
import {retirementRetention, unresolvedTitleRetirement} from './features/retirement_retention.mjs';
import {triggerAlwaysOn} from './features/trigger_always_on.mjs';
import {suggestionsCase} from './features/suggestions.mjs';
import {suggestionDraftBrowser} from './features/suggestion_draft_browser.mjs';
import {sharedIngressReceipt} from './features/conversation_ingress.mjs';
import {generatedTitleCustody, titleCustodyAfterRestart} from './features/title_custody.mjs';
import {competingRenames, renameAfterPublicationAbort} from './features/rename_contention.mjs';
import {accountAppearance, accountBuildVersion, accountDocumentation, accountSignIn, accountSignOut,
  accountTextSize, accountOperationsDenied, gatewayDocumentation, gatewayNavigation, legacyNavigation} from './features/account_browser.mjs';
import {agentOpen, agentRename, commandSearch, copyAgentResponse, taskDraft, taskNavigation} from './features/task_navigation.mjs';

const root = dirname(fileURLToPath(import.meta.url));
// Synthetic credentials travel over the parent's pipe, never a config file or argv.
const config = await readConfig(process.stdin);
const inventory = await loadInventory(join(root, 'inventory'));
const mappings = JSON.parse(await readFile(join(root, 'case-mapping.json'), 'utf8'));
const writerRegistry = JSON.parse(await readFile(join(root, 'writer-regressions.json'), 'utf8'));
await mkdir(config.matrix_output, {recursive: true, mode: 0o700});
const rows = [], observations = [], adminOwnerProofs = [], apiObservations = [], savedDocuments = [];
let phase, activeBrowserCase;
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
  const failures = [];
  const recordFailure = async (stage, error) => {
    const receipt = await retainFailure(config.matrix_output, stage, error, {case: name, phase, iteration});
    failures.push(receipt);
  };
  try { const value = await operation(recordFailure); row.verdict = 'PASS'; return value; }
  catch (error) {
    row.verdict = 'FAIL';
    if (!failures.length) await recordFailure('primary', error).catch(() => {});
    throw error;
  }
  finally {
    if (failures.length) row.document_failures = failures;
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
  reserveResumePair: () => requestPacer.wait(config.admin_bearer, 'POST', 2),
  withholdAdmissionReply: true,
  request: (role, am, method, path, options) => {
    if (options.discardSuccessfulReply) {
      check(role === 'admin' && method === 'POST' && /^\/api\/v1\/admin\/admission\/(stop|resume)$/.test(path),
        'admission_reply_discard_route_unexpected');
    }
    if (options.pacedPair) {
      check(role === 'admin' && method === 'POST' && path === '/api/v1/admin/admission/resume',
        'admission_resume_pair_route_unexpected');
      return admin.rawRequest(am, method, path, options);
    }
    return (role === 'admin' ? admin : user).request(am, method, path, options);
  },
  record: async row => {
    const scoped = row.feature === 'sandbox_always_on_cross_am';
    const ids = row.requests.map(request => request.request_id).filter(Boolean);
    const proofs = adminOwnerProofs.filter(proof => ids.includes(proof.request_id));
    await record({case: 'admin.' + row.feature, verdict: row.verdict,
    phase, iteration: row.iteration, elapsed_ms: row.elapsed_ms, error_code: row.error_code,
    primary_error_code: row.primary_error_code, restoration_error_code: row.restoration_error_code,
    failures: row.failures,
    ams: [...new Set(row.requests.map(x => x.am))],
    non_owner: scoped && ids.length === row.requests.length && proofs.length === ids.length
      && proofs.every(proof => proof.verdict === 'PASS' && proof.owner !== proof.ingress),
    routing: scoped ? 'agent_bound_sandbox_non_owner' : 'global_admin_has_no_agent_owner',
    requests: row.requests, owner_proofs: proofs});
    if (row.owner_relinquishment) {
      const proof = row.owner_relinquishment;
      await record({case: 'writer.lifecycle_owner_relinquishment_browser', verdict: row.verdict, phase,
        iteration: row.iteration, ams: [proof.ingress, proof.owner], real_browser: true, owner_relinquishment: proof});
    }
  }});
const result = {schema: 'multi-am-features-v1', verdict: 'RUNNING', source: config.expected_api_source,
  production_acceptance: false, image: config.expected_api_image, started_at: new Date().toISOString()};

async function withBrowser(am_origins, run, name) {
  const browser = await openPreparedBrowser({...config, am_origins, observeBrowserRateLimit(am, evidence) {
    if (activeBrowserCase?.am === am) activeBrowserCase.rateLimit ??= evidence;
  }});
  let outcome;
  await withRestoration(async () => { outcome = await run(browser); }, () => browser.close(),
    (stage, error) => retainFailure(config.output_dir, stage, error, {case: name}));
  return outcome;
}

async function browserCase(am, iteration, name, run) {
  const start = performance.now();
  const row = {case: name, phase, iteration, ams: [am], real_browser: true, verdict: 'RUNNING'};
  const observation = {am};
  activeBrowserCase = observation;
  try { await run(); row.verdict = 'PASS'; }
  catch (error) {
    row.verdict = 'FAIL';
    row.browser_failure = error?.name === 'TimeoutError' ? 'timeout' : 'assertion_or_transport_error';
    if (error?.response?.status === 429) {
      const {timing, retry_after: after, body} = error.response;
      row.rate_limit = rateLimitEvidence(timing?.method, timing?.route, after, body?.code);
    } else if (observation.rateLimit) {
      row.rate_limit = observation.rateLimit;
    }
    throw error;
  }
  finally {
    activeBrowserCase = undefined;
    row.elapsed_ms = performance.now() - start;
    await record(row);
  }
}

async function accountBrowserCases(ams, witness) {
  const cases = [['user.account_appearance', accountAppearance], ['user.account_text_size', accountTextSize],
    ['user.account_build_version', page => accountBuildVersion(page, config.expected_api_source)],
    ['user.navigation_admin_denied', page => accountOperationsDenied(page, config.owner_id)],
    ['user.account_documentation', page => accountDocumentation(page, witness.agentId)],
    ['user.navigation_legacy_routes', legacyNavigation],
    ['user.navigation_tasks', page => taskNavigation(page, witness)],
    ['user.navigation_eg_transition', page => gatewayNavigation(page, witness, config.owner_id)],
    ['user.task_draft', page => taskDraft(page, witness, {owner: config.owner_id,
      recipient: 'native-am-fixture-admin', recipientCookie: config.recipient_browser_session_cookie})]];
  if (config.eg_origin) cases.push(['eg.public_documentation', gatewayDocumentation]);
  await withBrowser(ams, async browser => {
    for (let iteration = 1; iteration <= 3; iteration++) {
      for (const am of ['a', 'b']) {
        await browser.forIngress(am, async ({page}) => {
          for (const [name, run] of cases) {
            await browserCase(am, iteration, name, () => run(page));
          }
          let document;
          await browserCase(am, iteration, 'user.documents_browser_create_after_lost_reply', async () => {
            document = await createDocumentAfterLostReply({page, ingress: am, user, ownerId: config.owner_id});
          });
          await browserCase(am, iteration, 'user.documents_browser_edit_and_conflict', () =>
            documentEditAndConflict({page, ingress: am, user, document}));
          await browserCase(am, iteration, 'user.documents_browser_people_sharing', () => documentPeople({
            page, ingress: am, owner: user, recipient: admin, documentId: document.id, recipientId: 'native-am-fixture-admin',
            recordFailure: (stage, error) => retainFailure(config.matrix_output, stage, error,
              {case: 'user.documents_browser_people_sharing', phase, iteration}),
          }));
          if (iteration === 1) await browserCase(am, iteration, 'user.documents_browser_comment', () =>
            documentCommentBrowser({page, user, ingress: am, documentId: document.id, ownerId: config.owner_id,
              recordFailure: (stage, error) => retainFailure(config.matrix_output, stage, error,
                {case: 'user.documents_browser_comment', phase, iteration})}));
          await browserCase(am, iteration, 'user.documents_browser_browse', () => documentBrowse({page, ingress: am,
            user, admin, ownerId: config.owner_id, recordFailure: (stage, error) =>
              retainFailure(config.matrix_output, stage, error, {case: 'user.documents_browser_browse', phase, iteration})}));
        });
        await browser.forIngress(am, async ({page}) => {
          const peer = am === 'a' ? 'b' : 'a';
          await browserCase(am, iteration, 'user.account_sign_in', () => accountSignIn(page, config.owner_id));
          await browserCase(am, iteration, 'user.account_sign_out', () => accountSignOut(page, ams[peer], config.host));
        }, {freshSession: true});
      }
    }
  }, 'account_browser');
}

async function heldCases(context, exercise, repetitions = 3) {
  const previousRecorder = context.api.router.record;
  context.api.router.record = async proof => { observations.push(proof); await previousRecorder(proof); };
  try {
    for (let iteration = 1; iteration <= repetitions; iteration++) {
      let activeRow;
      await exercise({...context, recordState: async (stage, state) => activeRow.states.push({stage, state}),
        runCase: async (name, operation) => {
          const start = performance.now(), first = observations.length;
          const row = {case: name, phase: context.phase, iteration, verdict: 'RUNNING', states: []};
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
              && row.owner_proofs.every(proof => proof.verdict === 'PASS' && proof.owner !== proof.ingress);
            row.elapsed_ms = performance.now() - start;
            await record(row);
          }
        }});
    }
  } finally { context.api.router.record = previousRecorder; }
}

try {
  result.journey = await runJourney(config, {navigationHook: async ({config: journeyConfig, active, retired}) => {
    check(retired.deleted === true, 'browser_search_retirement_unproven');
    Object.assign(normal.ams, journeyConfig.ams);
    await withBrowser(journeyConfig.ams, async browser => {
      for (const am of ['a', 'b']) {
        const row = {case: 'user.command_search_retirement', phase: 'after_restart', iteration: 1,
          ams: [am], real_browser: true, verdict: 'RUNNING'};
        try {
          await browser.forIngress(am, ({page}) => commandSearch(page, active, retired));
          row.verdict = 'PASS';
        } catch (error) { row.verdict = 'FAIL'; throw error; }
        finally { await record(row); }
      }
    }, 'command_search_after_retirement');
  }, retirementHook: async context => {
    Object.assign(normal.ams, context.config.ams);
    const retired = await retirementRetention({...context, user, outsider});
    await context.step('unresolved_title_retirement_defers_purge', () =>
      unresolvedTitleRetirement({...context, recordFailure: (stage, error) =>
        retainFailure(config.matrix_output, stage, error, {case: 'unresolved_title_retirement_defers_purge'})}));
    return retired;
  }, triggerHook: context => {
    Object.assign(normal.ams, context.config.ams);
    return withBrowser(context.config.ams, browser => triggerAlwaysOn({...context, user, admin, outsider, browser}),
      'trigger_always_on_browser');
  }, queuedTurnHook: context => heldCases(context, async held => {
    if (context.phase === 'after_restart') await busyConfigurationCase(held);
    await held.runCase('writer.queued_edit_cancel', () => queuedCommandCase(held));
  }), heldTurnHook: context => heldCases(context, heldConfigurationCases), featureHook: async context => {
    phase = context.phase;
    await heldCases(context, value => value.runCase('writer.automatic_title_custody', () =>
      phase === 'before_restart' ? generatedTitleCustody({...value,
        recordFailure: (stage, error) => retainFailure(config.matrix_output, stage, error, {case: 'writer.automatic_title_custody', phase})})
        : titleCustodyAfterRestart(value)), 1);
    // The same admin groups run before and after actual owner loss. Their public
    // mutations use CAS, peer readback, stale-revision refusal and restoration.
    Object.assign(normal.ams, context.config.ams);
    await accountBrowserCases(context.config.ams, context.record);
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
    await matrix.run({iterations: 3, groups: ['reads', 'denials', 'settings', 'triggers', 'admission', 'usage'],
      verifyRuntimeInventory: async (iteration, ...inventories) => {
        const {adapter, record} = context;
        const observation = await adapter.runtimeFor({agentId: record.agentId});
        check(observation.agent_id === record.agentId && observation.sandbox_id === record.sandboxId
          && observation.runtime_id === record.runtime.runtime_id
          && observation.pod_uid === record.runtime.pod_uid && Number.isSafeInteger(observation.runtime_generation)
          && observation.runtime_generation > 0 && observation.provider_status === 'ready',
        'runtime_inventory_observer_unbound');
        verifyRuntimeInventory(observation, inventories);
        if (iteration === 1) {
          const unavailable = phase === 'before_restart' ? 'a' : 'b';
          const peer = unavailable === 'a' ? 'b' : 'a';
          const route = '/api/v1/admin/runtime-inventory';
          const setAuthority = async allow => {
            const proof = await adapter.runtimeInventoryAuthority({manager: unavailable, allow});
            check(proof?.manager === unavailable && proof.allowed === allow && proof.peer_allowed === true,
              'runtime_inventory_authority_unbound');
          };
          await matrix.withRestoration(async () => {
            await setAuthority(false);
            const refused = await matrix.call(unavailable, 'GET', route, undefined, [503]);
            check(refused?.code === 'operator_service_unavailable' && !Object.hasOwn(refused, 'runtimes'),
              'runtime_inventory_unavailability_untyped');
            verifyRuntimeInventory(observation, [await matrix.call(peer, 'GET', route)]);
          }, async () => {
            await setAuthority(true);
            verifyRuntimeInventory(observation, [await matrix.call(unavailable, 'GET', route)]);
          });
        }
      },
      verifyAdmission: async (ingress, mode) => {
        const {record, create, cleanup} = context;
        check(typeof record.workspaceId === 'string' && record.workspaceId.length > 0, 'admission_probe_workspace_missing');
        const existing = (await user.rawRequest(ingress, 'GET', '/api/v1/agents/' + record.agentId)).body;
        check(existing?.id === record.agentId && existing.status === 'ready'
          && existing.workspace?.id === record.workspaceId && existing.sandbox?.id === record.sandboxId,
        'admission_probe_existing_agent_changed');
        let child;
        try {
          child = await create(ingress, false, {workspace_id: record.workspaceId});
        } catch (error) {
          if (mode === 'paused' && error.response?.status === 503 && error.response.body?.code === 'admission_paused'
            && error.response.timing?.method === 'POST' && error.response.timing.route === '/api/v1/agents') return;
          throw error;
        }
        await cleanup(child);
        check(mode === 'open', 'admission_stop_allowed_fresh_agent');
        check(child.agentId !== record.agentId && child.sandboxId === record.sandboxId
          && child.workspaceId === record.workspaceId && child.deleted === true, 'admission_resume_probe_lineage_unbound');
      }});
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
          await caseContext.runCase('writer.skill_publication_retry', () => installSkillCase(caseContext, true));
          await caseContext.runCase('writer.share_and_artifact_receipts', async () => {
            let child;
            await withRestoration(async () => {
              child = await context.create('b', false, {workspace_id: context.record.workspaceId});
              check(child.agentId !== context.record.agentId && child.workspaceId === context.record.workspaceId
                && child.sandboxId === context.record.sandboxId, 'artifact_fixture_child_lineage_unbound');
              await shareAgentArtifact({...caseContext, record: child, user, outsider});
            }, async () => { if (child) await context.cleanup(child); }, caseContext.recordFailure);
          });
          await caseContext.runCase('writer.artifact_unknown_publication',
            () => artifactUnknownPublication({...caseContext, user}));
          await caseContext.runCase('writer.artifact_committed_without_acknowledgement',
            () => artifactCommittedWithoutAcknowledgement({...caseContext, user}));
          await caseContext.runCase('writer.share_revoked_before_redemption_retry',
            () => revokeWhileRedemptionRetries({...caseContext, user, outsider}));
          await caseContext.runCase('writer.organization_share_membership',
            () => organizationShare({...caseContext, user, outsider}));
          await caseContext.runCase('writer.agent_access_request_denial',
            () => accessRequest({...caseContext, user, outsider}));
          await caseContext.runCase('writer.agent_access_request_approval',
            () => accessRequest({...caseContext, user, outsider}, true));
        }
        await userCases({...caseContext, outsider});
        if (iteration === 1) {
          await caseContext.runCase('writer.competing_renames', () => competingRenames(caseContext));
          await caseContext.runCase('writer.rename_publication_retry', () => renameAfterPublicationAbort(caseContext));
          await caseContext.runCase('writer.competing_system_prompts', () => competingSystemPrompts(caseContext));
          let transcriptWitness;
          await caseContext.runCase('writer.shared_conversation_ingress', async () => {
            transcriptWitness = await sharedIngressReceipt(caseContext);
          });
          await withBrowser(context.config.ams, async browser => {
            for (const am of ['a', 'b']) {
              await browser.forIngress(am, async ({page}) => {
                await browserCase(am, iteration, 'user.agent_open', () => agentOpen(page, context.record, transcriptWitness));
                await browserCase(am, iteration, 'user.agent_copy_response', () => copyAgentResponse(page, transcriptWitness));
                await browserCase(am, iteration, 'user.agent_rename', () => agentRename({page, ingress: am, api: user,
                  agentId: context.record.agentId, ...transcriptWitness, recordFailure: (stage, error) =>
                    retainFailure(config.matrix_output, stage, error, {case: 'user.agent_rename', phase, iteration})}));
                await browserCase(am, iteration, 'user.browser_file_roundtrip', () => browserFileRoundtrip({page,
                  api: user, outsider, ingress: am, agentId: context.record.agentId,
                  seedOversizeFile: directory => context.adapter.seedOversizeFile({
                    directory, agentId: context.record.agentId, sandboxId: context.record.sandboxId,
                    operationId: context.record.operationId,
                  }), recordFailure: (stage, error) =>
                    retainFailure(config.matrix_output, stage, error, {case: 'user.browser_file_roundtrip', phase, iteration})}));
              });
            }
          }, 'user.agent_open');
          await withChangedFile({...caseContext, api: user}, witness =>
            withBrowser(context.config.ams, async browser => {
              for (const am of ['a', 'b']) {
                await browser.forIngress(am, ({page}) => browserCase(am, iteration, 'user.browser_changed_file_preview',
                  async () => {
                    await agentOpen(page, context.record, witness);
                    await browserChangedFilePreview(page, witness);
                  }));
              }
            }, 'user.browser_changed_file_preview'));
          await withBrowser(context.config.ams, async browser => {
            for (const am of ['a', 'b']) {
              let witness;
              await caseContext.runCase('writer.suggestions_publish_once', async () => {
                witness = await suggestionsCase(caseContext, false);
              });
              await browser.forIngress(am, ({page}) => browserCase(am, iteration, 'user.agent_suggestions', () =>
                suggestionDraftBrowser({page, api: user, adapter: context.adapter,
                  agentId: context.record.agentId, witness})));
            }
          }, 'user.agent_suggestions');
          await caseContext.runCase('writer.suggestions_expire_after_rename', () => suggestionsCase(caseContext, true));
        }
        await configurationDenials({...caseContext, outsider});
        await configurationCases(caseContext);
        if (iteration === 1) await configurationCases(caseContext, true);
        await configurationCases({...caseContext, api: admin, conversationApi: context.api,
          runCase: (name, operation) => caseContext.runCase(name.replace('user.', 'admin.'), operation)});
        await matrix.sandboxAlwaysOn(iteration, {
          created_by_run: config.run_id, sandbox_id: context.record.sandboxId,
          verifyBrowserRelinquishment: iteration === 1 ? revision => withBrowser(context.config.ams,
            browser => relinquishAndReplayAlwaysOn({browser, user, revision, adapter: context.adapter,
              agentId: context.record.agentId, sandboxId: context.record.sandboxId}),
            'lifecycle_always_on_browser') : undefined,
          verifyAdmission: async stage => {
            const path = '/api/v1/agents/' + context.record.agentId;
            for (const am of ['a', 'b']) {
              const {body: detail} = await context.api.rawRequest(am, 'GET', path);
              check(detail.id === context.record.agentId && detail.sandbox?.id === context.record.sandboxId
                && detail.status === 'ready', 'always_on_public_agent_unavailable_' + stage);
              const {body: provenance} = await context.api.rawRequest(am, 'GET', path + '/provenance');
              check(provenance?.agent_id === detail.id && provenance.creation_origin === 'manual'
                && provenance.details?.trigger == null, 'always_on_lifecycle_provenance_changed');
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
  const trigger = result.journey?.steps.find(s => s.name === 'trigger_origin_always_on_and_replay' && s.verdict === 'PASS');
  const recovery = result.journey?.steps.find(s => s.name === 'actual_owner_kill_and_recovery' && s.verdict === 'PASS');
  const drain = result.journey?.steps.find(s => s.name === 'owner_drain_and_readiness' && s.verdict === 'PASS');
  const runtimeLoss = result.journey?.steps.find(s => s.name === 'native_runtime_loss_preserves_agents' && s.verdict === 'PASS');
  const retirement = result.journey?.steps.find(s => s.name === 'retirement_retains_terminal_evidence' && s.verdict === 'PASS');
  const unresolvedTitle = result.journey?.steps.find(s => s.name === 'unresolved_title_retirement_defers_purge'
    && s.verdict === 'PASS');
  const interrupts = ['plain_send_and_interrupt_non_owner', 'plain_send_and_interrupt_after_restart']
    .map(name => result.journey?.steps.find(step => step.name === name && step.verdict === 'PASS'))
    .filter(Boolean)
    .map(step => ({step: step.name, send_ingress: step.ingress_am, send_owner: step.send_owner,
      send_owner_epoch: step.send_owner_epoch, send_request_sha256: step.send_request_sha256,
      cancel_ingress: step.interrupt_am, cancel_owner: step.cancel_owner, cancel_owner_epoch: step.cancel_owner_epoch,
      cancel_request_sha256: step.cancel_request_sha256, command_sha256: step.command_sha256,
      response_sha256: step.response_sha256, terminal_ms: step.terminal_ms}));
  const journeyVerdict = result.journey?.verdict === 'LOCAL_TWO_AM_JOURNEY_PASS' ? 'PASS' : 'FAIL';
  const failedCase = rows.find(row => row.verdict === 'FAIL');
  // This allowlist is the only suite content eligible for CI artifact upload.
  await writeFile(join(config.summary_dir, 'coverage.json'), JSON.stringify({
    ...result.coverage, suite_verdict: result.verdict, source: result.source,
    journey_verdict: result.journey ? journeyVerdict : 'NOT_RUN',
    ...journeyFailureSummary(result.journey),
    first_failed_case: failedCase ? {case: failedCase.case, phase: failedCase.phase, iteration: failedCase.iteration,
      ...(failedCase.document_failures ? {document_failures: failedCase.document_failures} : {}),
      ...(failedCase.rate_limit ? {rate_limit: failedCase.rate_limit} : {}),
      ...(failedCase.browser_failure ? {browser_ingress: failedCase.ams[0], browser_failure: failedCase.browser_failure} : {})} : null,
    writer_coverage: result.writer_coverage,
    case_count: rows.length, non_owner_request_count: observations.length,
    owner_restart_pass: Boolean(recovery), owner_restart_replay: recovery?.replay ?? null,
    non_owner_interrupts: interrupts,
    retirement: retirement ? {retained_history: retirement.retained_history,
      eligible_purge: retirement.eligible_purge, retained_title_custody: retirement.retained_title_custody,
      retained_creation_provenance: retirement.retained_creation_provenance, purged_agents: retirement.purged_agents} : null,
    unresolved_title_retirement: unresolvedTitle ? {deferred_purge: unresolvedTitle.unresolved_title_deferred_purge,
      title_request_count: unresolvedTitle.title_request_count, eligible_purge: unresolvedTitle.eligible_purge,
      retained_creation_provenance: unresolvedTitle.retained_creation_provenance} : null,
    owner_drain: drain ? {readiness_dropped: drain.readiness_dropped, process_still_live: drain.process_still_live,
      peer_advanced: drain.peer_advanced, drain_ms: drain.drain_ms, takeover_ms: drain.takeover_ms,
      useful_reads_during_restart: drain.useful_reads_during_restart} : null,
    runtime_loss: runtimeLoss ? {recovered_agents: runtimeLoss.recovered_agents,
      known_recovery_aborts: runtimeLoss.known_recovery_aborts,
      competing_metadata_publications: runtimeLoss.competing_metadata_publications,
      overlapping_transaction_sha256: runtimeLoss.overlapping_transaction_sha256,
      recovery_abort_manager: runtimeLoss.recovery_abort_manager,
      recovery_abort_instance_sha256: runtimeLoss.recovery_abort_instance_sha256,
      recovery_commit_manager: runtimeLoss.recovery_commit_manager,
      recovery_commit_instance_sha256: runtimeLoss.recovery_commit_instance_sha256,
      metadata_writer_manager: runtimeLoss.metadata_writer_manager,
      metadata_writer_instance_sha256: runtimeLoss.metadata_writer_instance_sha256,
      metadata_ingress_manager: runtimeLoss.metadata_ingress_manager,
      metadata_ingress_instance_sha256: runtimeLoss.metadata_ingress_instance_sha256,
      metadata_request_sha256: runtimeLoss.metadata_request_sha256,
      generation_retirements: runtimeLoss.generation_retirements, post_loss_replies: runtimeLoss.post_loss_replies,
      previous_pod_sha256: runtimeLoss.previous_pod_sha256, replacement_pod_sha256: runtimeLoss.replacement_pod_sha256} : null,
    trigger_owner_relinquishment: trigger?.owner_relinquishment ?? null,
    trigger_orchestration: trigger?.orchestration ?? null,
    lifecycle_owner_relinquishments: rows.filter(row => row.case === 'writer.lifecycle_owner_relinquishment_browser'
      && row.verdict === 'PASS').map(row => ({phase: row.phase, ...row.owner_relinquishment})),
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
