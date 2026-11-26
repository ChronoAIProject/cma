import {readFile, writeFile, appendFile, mkdir} from 'node:fs/promises';
import {pathToFileURL} from 'node:url';
import {resolve, join} from 'node:path';
import {randomUUID} from 'node:crypto';
import {performance} from 'node:perf_hooks';
import {API, check, hash, id, normalizeConfig, runtimeBindings, assertRuntimeBindings, poll, other, assertOwner, assertRecovery, assertResponse, assertOracle, observeServingDuringRestart, observeTwoAmServingHold, awaitInitialIdle, wait} from './core.mjs';
import {NonOwnerRouter} from '../routing.mjs';
import {desktopFrame, closeDesktop} from './desktop.mjs';

const terminal = value => ['completed', 'failed', 'incomplete', 'cancelled'].includes(value.status);
const lineage = (record, detail) => detail?.id === record.agentId && detail.sandbox?.id === record.sandboxId
  && [detail.title, detail.owner_title, detail.worktree?.name, detail.sandbox?.name]
    .some(name => name === record.name || (record.writerTitle && name === record.writerTitle.title));

export async function runJourney(raw, {adoptionHook, featureHook, heldTurnHook, queuedTurnHook} = {}) {
  const config = normalizeConfig(raw);
  check(config.adapter_module && config.output_dir && config.owner_id, 'config_missing');
  let runtimes = runtimeBindings(config);
  check(typeof config.expected_api_image === 'string' && /@sha256:[a-f0-9]{64}$/.test(config.expected_api_image), 'exact_api_image_required');
  const apiSource = config.expected_api_source;
  const apiRelease = 'ci-checkout';
  check(/^[a-f0-9]{40}$/.test(apiSource ?? ''), 'exact_api_source_required');
  await mkdir(config.output_dir, {recursive: true, mode: 0o700});
  // Single-use output prevents a second run silently replacing its predecessor.
  await writeFile(join(config.output_dir, 'client-start.json'), JSON.stringify({at: new Date().toISOString(), pid: process.pid}), {flag: 'wx', mode: 0o600});
  const module = await import(pathToFileURL(resolve(config.adapter_module)));
  const adapter = await (module.createAdapter ?? module.default)(raw);
  for (const name of ['topology', 'runtimeFor', 'prepareTurn', 'ownerOf', 'killAm', 'releaseTurn', 'oracle', 'restoreAm']) check(typeof adapter[name] === 'function', 'adapter_missing_' + name);
  const result = {schema: 'ws4-two-am-journey-v1', scope: 'LOCAL_TWO_AM_SHARED_DATABASE', verdict: 'RUNNING',
    started_at: new Date().toISOString(), api_source: apiSource, api_image: config.expected_api_image, api_release: apiRelease,
    runtime_sources: {...config.runtime_sources}, runtime_images: {...config.runtime_images}, runtime_bindings: runtimes, production_acceptance: false,
    population_p95_qualified: false, owner_sha256: hash(config.owner_id), steps: [], requests: [], resources: [], cleanup: []};
  const api = new API(config, observation => result.requests.push(observation));
  api.router = new NonOwnerRouter({adapter, record: async proof => {
    (result.non_owner_requests ??= []).push(proof);
  }});
  const records = []; const turns = []; let killedAm = null;
  const evidencePath = join(config.output_dir, 'turn-evidence.private.jsonl');
  await writeFile(evidencePath, '', {flag:'wx', mode:0o600});
  result.turn_evidence_path = evidencePath;
  const retainTurn = (type, observation) => appendFile(evidencePath,
    JSON.stringify({at:new Date().toISOString(), type, ...observation})+'\n', {mode:0o600});
  async function observeOracle(caseId) {
    const value = await adapter.oracle({caseId});
    await retainTurn('provider_oracle', {case_id:caseId, value});
    return value;
  }
  const save = () => writeFile(join(config.output_dir, 'result.json'), JSON.stringify(result, null, 2) + '\n', {mode: 0o600});
  async function step(name, operation) {
    const started = performance.now(); const at = new Date().toISOString();
    try { const value = await operation(); result.steps.push({name, verdict: 'PASS', at, elapsed_ms: performance.now() - started, ...value}); await save(); return value; }
    catch (error) {result.steps.push({name, verdict: 'FAIL', at, elapsed_ms: performance.now() - started, error: safeError(error)}); await save(); throw error;}
  }
  async function detail(am, record) {
    const r = await api.request(am, 'GET', `/api/v1/agents/${record.agentId}`);
    if (!lineage(record, r.body)) {const error = new Error('agent_lineage_mismatch'); error.response = r; throw error;}
    return r.body;
  }
  async function create(am, desktop) {
    const record = {name: 'e2e-slo-ws4-' + randomUUID(), key: randomUUID(), desktop, am}; records.push(record);
    const start = performance.now();
    record.createBody = {name: record.name, machine_included: desktop, desktop,
      ...(desktop ? {initial_message: 'Report local readiness briefly without using tools or changing files.'} : {})};
    const receipt = await api.request(am, 'POST', '/api/v1/agents', {
      body: record.createBody, key: record.key, allowed: [202]});
    check(id(receipt.body?.id), 'lifecycle_receipt_invalid'); record.operationId = receipt.body.id;
    const op = await poll(async () => {
      const {body} = await api.request(other(am), 'GET', `/api/v1/agent-lifecycle/${record.operationId}`);
      check(body?.id === record.operationId, 'lifecycle_identity_mismatch');
      if (id(body.agent_id)) record.agentId = body.agent_id;
      if (['failed', 'waiting_for_connection'].includes(body.status)) {
        const failurePath = join(config.output_dir, 'create-failure-' + hash(record.operationId) + '.private.json');
        await writeFile(failurePath, JSON.stringify(body, null, 2) + '\n', {mode: 0o600, flag: 'wx'});
        result.create_failure = {operation_sha256: hash(record.operationId),
          agent_sha256: record.agentId ? hash(record.agentId) : null,
          lifecycle_status: body.status, native_response_path: failurePath};
        await save();
      }
      check(!['failed', 'waiting_for_connection'].includes(body.status), 'create_operation_failed'); return body;
    }, value => value.status === 'succeeded' && id(record.agentId), {timeout: 420000, code: 'create_lifecycle_timeout'});
    let ready;
    const readinessObservations = [];
    try {
      ready = await poll(async () => {
        const {body} = await api.request(other(am), 'GET', `/api/v1/agents/${record.agentId}`);
        readinessObservations.push({at:new Date().toISOString(), body});
        check(body?.id === record.agentId, 'created_agent_mismatch');
        if (id(body.sandbox?.id)) record.sandboxId = body.sandbox.id;
        check(!['failed', 'deleted'].includes(body.status), 'created_agent_failed'); return body;
      }, value => value.status === 'ready' && value.sandbox?.status === 'active', {timeout: 180000, code: 'agent_ready_timeout'});
    } catch (error) {
      const failurePath = join(config.output_dir, 'create-readiness-failure-' + hash(record.agentId) + '.private.json');
      await writeFile(failurePath, JSON.stringify({am:other(am), observations:readinessObservations,
        error:safeError(error), error_response:error.response ?? null}, null, 2)+'\n', {mode:0o600,flag:'wx'});
      const last = error.response?.body ?? readinessObservations.at(-1)?.body;
      result.create_readiness_failure = {am:other(am), agent_sha256:hash(record.agentId), status:last?.status ?? null,
        sandbox_status:last?.sandbox?.status ?? null, first_message_state:last?.first_message?.state ?? null,
        failure_class:last?.failure?.class ?? null, native_response_path:failurePath};
      await save(); throw error;
    }
    check(lineage(record, ready) && (desktop ? ready.desktop === true : ready.desktop !== true), 'created_lineage_variant_mismatch');
    record.ready = true;
    const runtime = await adapter.runtimeFor({agentId: record.agentId});
    const family = desktop ? 'omarchy' : 'base';
    check(runtime.agent_id === record.agentId || runtime.agent_sha256 === hash(record.agentId), 'runtime_agent_mismatch');
    check(runtime.sandbox_id === record.sandboxId || runtime.sandbox_sha256 === hash(record.sandboxId), 'runtime_sandbox_mismatch');
    check(runtime.image === config.runtime_images[family] && runtime.pod_uid && runtime.ready === true, 'runtime_image_ready_unbound');
    record.runtime = runtime;
    await poll(() => adapter.ownerOf({agentId: record.agentId}), owner => owner !== null, {timeout: 30000, code: 'initial_owner_missing'});
    api.routedAgents.add(record.agentId);
    api.bindAdmission(record.key, record.agentId);
    result.resources.push({name: record.name, desktop, agent_sha256: hash(record.agentId), sandbox_sha256: hash(record.sandboxId), operation_sha256: hash(record.operationId),
      create_to_ready_ms: performance.now() - start, lifecycle_status: op.status, runtime_family: family,
      runtime_source: runtimes[family].source, runtime_image: runtime.image,
      selected_runtime_image: runtimes[family].image, runtime_image_matches_selected: true,
      runtime_source_basis: 'Owner per-family artifact metadata joined to observed exact Pod image',
      runtime_pod_uid_sha256: hash(runtime.pod_uid), warm_claim: runtime.warm_claim ?? null});
    return record;
  }
  async function openBoth(record) {
    const routed = api.routingEnabled;
    api.routingEnabled = false; // These assertions prove both direct processes serve.
    try {
    for (const am of ['a', 'b']) {
      let last;
      const observed = []; const started = performance.now();
      try {
        const d = await awaitInitialIdle(async () => {
          last = await detail(am, record);
          observed.push({at: new Date().toISOString(), body: last});
          return last;
        });
        check(d.status === 'ready', 'open_not_ready');
        (result.open_readiness ??= []).push({am, agent_sha256: hash(record.agentId), elapsed_ms: performance.now()-started,
          observations: observed.map(x => ({at:x.at, status:x.body.status, first_message_state:x.body.first_message?.state ?? null,
            first_message_purpose:x.body.first_message?.purpose ?? null})), eventual_idle_ready:true});
      } catch (error) {
        last = error.response?.body ?? last;
        const failurePath = join(config.output_dir, 'open-failure-' + am + '-' + hash(record.agentId) + '.private.json');
        await writeFile(failurePath, JSON.stringify({observed, error_response:error.response ?? null, error:safeError(error)}, null, 2)+'\n', {mode:0o600,flag:'wx'});
        result.open_failure = {am, agent_sha256:hash(record.agentId), status:last?.status ?? null,
          sandbox_status:last?.sandbox?.status ?? null, failure_class:last?.failure?.class ?? null, native_response_path:failurePath};
        await save(); throw error;
      }
      const transcript = await api.request(am, 'GET', `/api/v1/agents/${record.agentId}/transcript`);
      check(transcript.body && typeof transcript.body === 'object', 'transcript_shape_invalid');
    }
    return {both_am_details_and_transcripts: true, agent_sha256: hash(record.agentId)};
    } finally { api.routingEnabled = routed; }
  }
  async function sendHeld(record, am, kind) {
    const caseId = 'ws4-' + kind + '-' + randomUUID(); const marker = 'WS4_' + randomUUID().replaceAll('-', '');
    await retainTurn('case_preparing', {case_id:caseId, kind, am, agent_id:record.agentId});
    await adapter.prepareTurn({caseId, kind, marker}); turns.push(caseId);
    const stream = await api.send(am, record.agentId, 'Return the exact marker ' + marker + ' without tools or file changes.', caseId);
    await retainTurn('send_receipt', {case_id:caseId, response_id:stream.responseId});
    const path = `/api/v2/agents/${record.agentId}/responses/${stream.responseId}/controls`;
    const controls = await poll(async () => {
      const {body} = await api.request(other(am), 'GET', path);
      check(body?.response?.receipt?.agent_id === record.agentId && body.response.receipt.response_id === stream.responseId, 'controls_receipt_mismatch');
      check(!terminal(body.response), 'held_turn_completed_early'); return body;
    }, value => value.target_active === true && Number.isSafeInteger(value.target?.turn_sequence), {timeout: 60000, code: 'held_turn_not_active'});
    await poll(() => observeOracle(caseId), value => {
      check(value.case_id === caseId && value.request_count <= 1, 'provider_duplicate_or_case_mismatch');
      return value.request_count === 1 && value.active_streams === 1 && value.completed_effect_count === 0;
    }, {timeout: 60000, code: 'runtime_provider_request_missing'});
    return {caseId, marker, stream, controls, path};
  }
  async function readResponse(am, record, responseId) {
    const {body} = await api.request(am, 'GET', `/api/v2/responses/${responseId}`); return assertResponse(body, record.agentId, responseId);
  }
  async function interrupt(record, am, phase) {
    const turn = await sendHeld(record, am, 'interrupt');
    if (queuedTurnHook && phase) await queuedTurnHook({record, api, adapter, config, turn, phase});
    check(turn.controls.runtime_support?.cancel === true, 'cancel_not_supported'); const start = performance.now();
    const {body, timing: controlTiming} = await api.request(other(am), 'POST', turn.path, {body: {turn_sequence: turn.controls.target.turn_sequence,
      expected_agent_revision: turn.controls.agent_revision, command: {type: 'cancel'}}, key: turn.caseId + '-cancel', allowed: [202]});
    await retainTurn('cancel_receipt', {case_id:turn.caseId, am:other(am), body});
    check(body?.target?.response_id === turn.stream.responseId && body.target.turn_sequence === turn.controls.target.turn_sequence
      && body.control?.receipt?.agent_id === record.agentId && id(body.control.receipt.command_id), 'cancel_receipt_mismatch');
    const commandId = body.control.receipt.command_id;
    await poll(async () => {
      const {body: command} = await api.request(am, 'GET', `/api/v2/commands/${commandId}`);
      await retainTurn('cancel_command', {case_id:turn.caseId, am, body:command});
      check(command.receipt?.command_id === commandId, 'cancel_command_mismatch');
      check(!['failed', 'cancelled'].includes(command.status), 'cancel_command_failed'); return command;
    }, x => x.status === 'completed', {timeout: 60000, code: 'cancel_delivery_timeout'});
    const response = await poll(async () => {
      const value = await readResponse(other(am), record, turn.stream.responseId);
      await retainTurn('interrupt_response', {case_id:turn.caseId, am:other(am), body:value});
      return value;
    }, terminal, {timeout: 60000, code: 'interrupt_terminal_timeout'});
    check(response.status === 'incomplete' && response.incomplete_details?.reason === 'interrupted', 'interrupt_not_confirmed');
    const oracle = await poll(() => observeOracle(turn.caseId), x => x.active_streams === 0, {timeout: 20000, code: 'interrupted_provider_not_closed'});
    assertOracle(oracle, turn.caseId, 0); turn.stream.abort(); await turn.stream.done;
    return {ingress_am: turn.stream.ingress_am, interrupt_am: controlTiming.am, response_sha256: hash(turn.stream.responseId), command_sha256: hash(commandId),
      interrupted: true, terminal_ms: performance.now() - start, provider_requests: oracle.request_count, provider_completions: 0};
  }
  async function recovery(record, am) {
    const turn = await sendHeld(record, am, 'owner-recovery');
    if (heldTurnHook) await heldTurnHook({record, api, adapter, config,
      phase: 'before_restart', turn});
    api.routingEnabled = false; // Survivor reads exercise the ordinary takeover path.
    const before = assertOwner(await adapter.ownerOf({agentId: record.agentId}), record.agentId);
    const kill = await adapter.killAm({agentId: record.agentId, owner: before}); killedAm = before.am;
    check(kill.killed_am === before.am && kill.actual_container_exited === true, 'actual_owner_exit_unproven');
    const survivor = other(before.am);
    const after = await poll(async () => {
      // The real public response read on the survivor triggers the normal recovery path.
      await readResponse(survivor, record, turn.stream.responseId);
      const current=await adapter.ownerOf({agentId: record.agentId});
      return current===null?null:assertOwner(current,record.agentId);
    }, value => value!==null && value.am === survivor && value.epoch > before.epoch, {timeout: 150000, code: 'owner_recovery_timeout'});
    assertRecovery(before, after, record.agentId);
    const held = await observeOracle(turn.caseId);
    check(held.request_count === 1 && held.completed_effect_count === 0 && held.active_streams === 1, 'runtime_turn_not_preserved');
    await adapter.releaseTurn({caseId: turn.caseId});
    const response = await poll(() => readResponse(survivor, record, turn.stream.responseId), terminal, {timeout: 90000, code: 'recovered_response_timeout'});
    check(response.status === 'completed', 'recovered_response_not_completed');
    check(JSON.stringify(response.output ?? []).includes(turn.marker), 'recovered_output_marker_missing');
    const oracle = await poll(() => observeOracle(turn.caseId), x => x.active_streams === 0, {timeout: 20000}); assertOracle(oracle, turn.caseId, 1);
    // Keep useful public reads on the surviving AM while its peer starts.
    // A successful restore alone cannot establish this concurrent-serving leg.
    const serving = await observeServingDuringRestart(() => adapter.restoreAm({am: killedAm}), async () => {
      const current = await detail(survivor, record);
      check(current.status === 'ready', 'survivor_not_ready_during_peer_startup');
      const transcript = await api.request(survivor, 'GET', `/api/v1/agents/${record.agentId}/transcript`);
      check(transcript.body && typeof transcript.body === 'object', 'survivor_transcript_missing_during_peer_startup');
    });
    const restored = serving.restored;
    if (restored?.origin) config.ams[killedAm] = restored.origin;
    check(restored?.ready === true, 'second_am_restore_failed'); killedAm = null;
    api.routingEnabled = true;
    const restoredTopology = await adapter.topology();
    assertRuntimeBindings(runtimes, restoredTopology);
    check(restoredTopology.ams[before.am].ready === true
      && restoredTopology.ams[before.am].pod_uid !== before.pod_uid
      && restoredTopology.ams[survivor].pod_uid === after.pod_uid
      && ['a','b'].every(am => restoredTopology.ams[am].image === config.expected_api_image), 'restored_topology_identity_mismatch');
    result.am_restart_while_peer_serves = {verdict:'PASS', restarted_am:before.am, serving_am:survivor,
      successful_detail_and_transcript_pairs:serving.servingSamples, elapsed_ms:serving.elapsed_ms,
      restored_pod_uid_sha256:hash(restoredTopology.ams[before.am].pod_uid), api_image:config.expected_api_image};
    await openBoth(record); turn.stream.abort(); await turn.stream.done;
    return {killed_am: before.am, recovered_am: after.am, owner_epoch_before: before.epoch, owner_epoch_after: after.epoch,
      owner_container_exit_observed: true, response_sha256: hash(turn.stream.responseId), provider_requests: 1, provider_completions: 1,
      unchanged_agent_and_response: true, second_am_restored: true};
  }
  async function cleanup(record) {
    if (!record.agentId || record.deleted) return;
    check(record.ready && record.sandboxId, 'cleanup_create_lineage_incomplete');
    const am = killedAm ? other(killedAm) : other(record.am);
    const receiptPath = join(config.output_dir, 'cleanup-' + hash(record.agentId) + '-' + randomUUID() + '.private.json');
    const journal = {am, agent_id:record.agentId, sandbox_id:record.sandboxId, started_at:new Date().toISOString(), mutations:[], last_reads:{}};
    const persist = () => writeFile(receiptPath, JSON.stringify(journal,null,2)+'\n', {mode:0o600});
    async function cleanupRequest(target, method, path, options) {
      try {
        const response = await api.request(target, method, path, options);
        const entry = {at:new Date().toISOString(), method, path, ...response};
        if (method === 'GET') journal.last_reads[path] = entry;
        else {journal.mutations.push(entry); await persist();}
        return response;
      } catch(error) {
        journal.request_error = {at:new Date().toISOString(), method, path, error:safeError(error), response:error.response ?? null};
        await persist(); throw error;
      }
    }
    async function cleanupDetail() {
      const response = await cleanupRequest(am, 'GET', `/api/v1/agents/${record.agentId}`);
      check(lineage(record, response.body), 'agent_lineage_mismatch'); return response.body;
    }
    try {
      const current = await cleanupDetail();
      const op = await cleanupRequest(am, 'GET', `/api/v1/agent-lifecycle/${record.operationId}`);
      check(op.body.id === record.operationId && op.body.agent_id === record.agentId, 'cleanup_lifecycle_mismatch');
      if (!['stopped', 'deleted'].includes(current.status)) {
        await cleanupRequest(am, 'POST', `/api/v1/agents/${record.agentId}/stop`, {body: {}, key: record.key + '-stop', allowed: [200, 202, 204]});
        await poll(() => cleanupDetail(), value => value.status === 'stopped', {timeout: 180000, code: 'agent_stop_timeout'});
      }
      await cleanupRequest(am, 'DELETE', `/api/v1/agents/${record.agentId}`, {key: record.key + '-delete', allowed: [200, 202, 204]});
      await poll(() => cleanupRequest(am, 'GET', `/api/v1/agents/${record.agentId}`, {allowed: [200, 404]}), value => value.status === 404 || value.body?.status === 'deleted', {timeout: 180000});
      const sandbox = await cleanupRequest(am, 'GET', `/api/v1/sandboxes/${record.sandboxId}`);
      check(sandbox.body?.id === record.sandboxId && sandbox.body.owner === config.owner_id, 'sandbox_owner_mismatch');
      const suffix = ' sandbox ' + record.operationId;
      check(sandbox.body.name === record.name.slice(0, Math.max(0, 64 - suffix.length)) + suffix, 'sandbox_operation_name_mismatch');
      let cursor; const seen = new Set();
      do {
        const query = new URLSearchParams({limit: '200', ...(cursor ? {cursor} : {})});
        const page = await cleanupRequest(am, 'GET', '/api/v1/agents?' + query);
        check(Array.isArray(page.body?.items), 'agent_inventory_missing');
        check(!page.body.items.some(x => x.sandbox_id === record.sandboxId && x.status !== 'deleted'), 'sandbox_has_live_agent');
        cursor = page.body.next; if (cursor) {check(typeof cursor === 'string' && !seen.has(cursor) && seen.size < 20, 'inventory_incomplete'); seen.add(cursor);}
      } while (cursor);
      await cleanupRequest(am, 'DELETE', `/api/v1/sandboxes/${record.sandboxId}`, {key: record.key + '-retire', allowed: [200, 202, 204]});
      await poll(() => cleanupRequest(am, 'GET', `/api/v1/sandboxes/${record.sandboxId}`, {allowed: [200, 404]}), value => value.status === 404 || value.body?.status === 'retired', {timeout: 300000, code: 'sandbox_retire_timeout'});
      record.deleted = true; result.cleanup.push({agent_sha256: hash(record.agentId), sandbox_sha256: hash(record.sandboxId), public_stop_delete_retire: 'PASS', am});
    } catch(error) {
      journal.failure = safeError(error); throw error;
    } finally {
      journal.finished_at = new Date().toISOString(); await persist();
      (result.cleanup_evidence ??= []).push({am, agent_sha256:hash(record.agentId), native_response_path:receiptPath,
        delete_status:journal.mutations.find(x => x.method === 'DELETE' && x.path === `/api/v1/agents/${record.agentId}`)?.status ?? null,
        last_agent_status:journal.last_reads[`/api/v1/agents/${record.agentId}`]?.body?.status ?? null,
        failure:journal.failure ?? null});
    }
  }
  async function desktop(record) {
    let socket;
    try {
      const first = await api.router.dispatch(record.agentId, async ingress => {
        const opened = await desktopFrame(api, ingress, record.agentId); socket = opened.socket; return opened;
      });
      const busy = await api.rawRequest(other(first.result.am), 'GET', `/api/v1/agents/${record.agentId}/desktop`, {allowed: [409]});
      check(busy.body?.code === 'desktop_busy' || busy.body?.error?.code === 'desktop_busy' || busy.body?.detail?.code === 'desktop_busy', 'desktop_busy_reason_unbound');
      await closeDesktop(socket); socket = null;
      // Session release is asynchronous; require discovery admission before opening A.
      await poll(() => api.rawRequest(other(first.result.am), 'GET', `/api/v1/agents/${record.agentId}/desktop`, {allowed: [200, 409]}), x => x.status === 200, {timeout: 30000});
      const second = await api.router.dispatch(record.agentId, async ingress => {
        const opened = await desktopFrame(api, ingress, record.agentId); socket = opened.socket; return opened;
      });
      return {frames: [first.result, second.result], cross_am_busy_refusal: true};
    } finally {if (socket) await closeDesktop(socket);}
  }
  async function holdBothServing(record) {
    const startedAt = new Date().toISOString();
    let observation;
    try { observation = await observeTwoAmServingHold({expectedImage:config.expected_api_image,
      topology: async () => {
        const value = await adapter.topology();
        await retainTurn('hold_topology', {value});
        assertRuntimeBindings(runtimes, value);
        return value;
      },
      read: async am => {
        const routed = api.routingEnabled;
        api.routingEnabled = false;
        try {
        const current = await detail(am, record);
        await retainTurn('hold_agent_detail', {am, body:current});
        check(current.status === 'ready' && current.sandbox?.status === 'active', 'hold_agent_not_idle_ready');
        const response = await api.request(am, 'GET', `/api/v1/agents/${record.agentId}/transcript`);
        await retainTurn('hold_transcript', {am, body:response.body});
        check(response.body && typeof response.body === 'object', 'hold_transcript_missing');
        } finally { api.routingEnabled = routed; }
      },
      sample: async progress => {
        result.serving_hold = {status:'RUNNING', started_at:startedAt, ...progress,
          first_success_at:result.serving_hold?.first_success_at ?? new Date().toISOString(),
          last_success_at:new Date().toISOString(),
          successful_detail_and_transcript_pairs_per_am:progress.samples, evidence_path:evidencePath};
        await save();
      }}); } catch (error) {
      await retainTurn('hold_failure', {error:safeError(error), response:error.response ?? null});
      result.serving_hold = {...result.serving_hold, status:'FAIL', started_at:startedAt,
        failed_at:new Date().toISOString(), failure:safeError(error), evidence_path:evidencePath};
      await save(); throw error;
    }
    result.serving_hold = {...result.serving_hold, status:'PASS', finished_at:new Date().toISOString(),
      processes:Object.fromEntries(Object.entries(observation.processes).map(([am, value]) => [am, {
        pod_uid_sha256:hash(value.pod_uid), instance_sha256:hash(value.instance_id),
        container_id_sha256:hash(value.container_id), started_at:value.started_at}]))};
    return result.serving_hold;
  }
  try {
    await step('two_am_shared_database', async () => {
      const t = await adapter.topology();
      assertRuntimeBindings(runtimes, t);
      check(t.coordination_enabled === true && t.shared_database === true, 'coordination_or_shared_database_unbound');
      check(t.ams?.a?.ready === true && t.ams?.b?.ready === true && t.ams.a.pod_uid !== t.ams.b.pod_uid, 'two_ready_am_pods_unbound');
      for (const am of ['a', 'b']) check(t.ams[am].image === config.expected_api_image, 'am_image_mismatch');
      return {coordination_enabled: true, shared_database: true, am_pod_uid_hashes: ['a', 'b'].map(am => hash(t.ams[am].pod_uid))};
    });
    if (adoptionHook) {
      let record;
      await step('rollback_desktop_create_b_ready_a', async () => {record = await create('b', true); return {agent_sha256: hash(record.agentId)};});
      await step('rollback_open_both', () => openBoth(record));
      let socket;
      try {
        const opened = await desktopFrame(api, 'b', record.agentId); socket = opened.socket;
        await step('rollback_on_adoption', async () => ({...opened.result, actual_frame: true}));
        // Private in-memory handles belong to the rollback owner. Never serialize
        // bearer/cookies/socket or raw fixture identities into this result.
        const value = await adoptionHook({agent_id: record.agentId, sandbox_id: record.sandboxId,
          operation_id: record.operationId, runtime: record.runtime, api, adapter, socket,
          config, closeDesktop: () => closeDesktop(socket), detail: () => detail('b', record)});
        check(value?.pass === true, 'rollback_owner_hook_failed');
        result.verdict = 'LOCAL_ROLLBACK_OWNER_HOOK_PASS';
      } finally {if (socket) await closeDesktop(socket);}
      return result;
    }
    let plain;
    await step('plain_create_a_ready_b', async () => {plain = await create('a', false); return {agent_sha256: hash(plain.agentId)};});
    await step('plain_open_both', () => openBoth(plain));
    if (featureHook) await step('features_before_restart', () => featureHook({record: plain, api, adapter, config, phase: 'before_restart'}));
    await step('plain_send_and_interrupt_non_owner', () => interrupt(plain, 'a', 'before_restart'));
    await step('actual_owner_kill_and_recovery', () => recovery(plain, 'b'));
    if (featureHook) await step('features_after_restart', () => featureHook({record: plain, api, adapter, config, phase: 'after_restart'}));
    await step('plain_send_and_interrupt_after_restart', () => interrupt(plain, 'b', 'after_restart'));
    await step('plain_public_stop_retire', async () => {await cleanup(plain); return {retired: true};});
    if (!runtimes.omarchy) {
      await step('await_omarchy_runtime_input', async () => {
        const ready = await poll(async () => {
          if (config.runtime_ready_file) return JSON.parse(await readFile(config.runtime_ready_file, 'utf8'));
          return adapter.topology();
        }, value => typeof value.runtime_images?.omarchy === 'string' && typeof value.runtime_sources?.omarchy === 'string', {timeout: 1800000, interval: 5000, code: 'omarchy_input_pending'});
        runtimes = assertRuntimeBindings(runtimes, ready);
        config.runtime_images = {...ready.runtime_images}; config.runtime_sources = {...ready.runtime_sources};
        result.runtime_images = {...config.runtime_images}; result.runtime_sources = {...config.runtime_sources};
        result.runtime_bindings = runtimes; return {...runtimes.omarchy};
      });
    }
    let machine;
    await step('desktop_create_b_ready_a', async () => {machine = await create('b', true); return {agent_sha256: hash(machine.agentId)};});
    await step('desktop_open_both', () => openBoth(machine));
    await step('desktop_frames_and_cross_am_admission', () => desktop(machine));
    await step('desktop_send_and_interrupt_non_owner', () => interrupt(machine, 'b'));
    await step('both_am_serving_hold_10_minutes', () => holdBothServing(machine));
    await step('desktop_public_stop_retire', async () => {await cleanup(machine); return {retired: true};});
    result.verdict = 'LOCAL_TWO_AM_JOURNEY_PASS';
  } catch (error) {
    result.verdict = 'FAIL'; result.failure = safeError(error); await save();
    if (error.response) await writeFile(join(config.output_dir, 'failure-response.private.json'),
      JSON.stringify(error.response, null, 2), {flag: 'wx', mode: 0o600});
    // The sole owner joins bounded read-only diagnostics before any stream
    // release or public retirement can remove the failing runtime evidence.
    if (typeof adapter.captureFailure === 'function') {
      try {
        const captured = await adapter.captureFailure({reason:result.failure});
        await retainTurn('owner_failure_capture', {result:captured});
        result.failure_capture = {attempted:captured.attempted === true, path:captured.path ?? null,
          error_count:Array.isArray(captured.errors) ? captured.errors.length : null};
      } catch (captureError) {
        result.failure_capture = {attempted:true, failure:safeError(captureError)};
      }
    } else result.failure_capture = {attempted:false, reason:'owner_hook_unavailable'};
  }
  finally {
    api.routingEnabled = false; // Cleanup must also handle expired or missing ownership.
    await api.close();
    for (const caseId of turns) {try {await adapter.releaseTurn({caseId});} catch {result.cleanup.push({provider_release: 'FAILED', case_id: caseId});}}
    if (killedAm) {try {const value = await adapter.restoreAm({am: killedAm}); if (value?.origin) config.ams[killedAm] = value.origin; check(value?.ready, 'restore_failed'); killedAm = null;} catch {result.cleanup.push({am_restore: 'FAILED'});}}
    for (const record of records) {try {await cleanup(record);} catch (error) {result.cleanup.push({agent_sha256: record.agentId ? hash(record.agentId) : null, public_cleanup: 'FAILED', reason: safeError(error)});}}
    if (result.verdict.endsWith('_PASS') && result.cleanup.some(x => Object.values(x).includes('FAILED'))) result.verdict = 'FAIL_CLEANUP';
    result.finished_at = new Date().toISOString(); result.infrastructure_release = 'OWNED_BY_SOLE_LAUNCHER'; await save();
  }
  return result;
}
export const withDesktopAdoption = (config, hook) => runJourney(config, {adoptionHook: hook});
function safeError(error) { return /^[a-zA-Z0-9_]+$/.test(error?.message ?? '') ? error.message : 'transport_or_adapter_error'; }
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {check(process.argv.length === 3, 'private_config_path_required'); const result = await runJourney(JSON.parse(await readFile(process.argv[2], 'utf8'))); console.log(JSON.stringify({verdict: result.verdict, failure: result.failure ?? null})); process.exitCode = result.verdict === 'LOCAL_TWO_AM_JOURNEY_PASS' ? 0 : 1;}
  catch (error) {console.error(safeError(error)); process.exitCode = 1;}
}
