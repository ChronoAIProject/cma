import {createHash, randomUUID} from 'node:crypto';
import {NonOwnerRouter} from '../routing.mjs';
import {latencyVerdict} from './latency.mjs';
import {validEgReceipt, externalPlatform, validPlatformProof, platformComplete} from './evidence-scope.mjs';

export class Blocked extends Error {
  constructor(code) { super(code); this.name = 'Blocked'; }
}
const check = (value, code) => { if (!value) throw Error(code); };
const safeCode = error => /^[a-z][a-z0-9_]{0,100}$/.test(error?.message ?? '')
  ? error.message : 'case_operation_failed';
const hash = value => createHash('sha256').update(value).digest('hex');
const errorCodes = (error, depth = 0) => [safeCode(error), ...(depth < 3 && Array.isArray(error?.errors)
  ? error.errors.slice(0, 8).flatMap(child => errorCodes(child, depth + 1)) : [])];

export async function executeCase(definition, row, environment) {
  const {phase, iteration, am, image, source, fixtures = {}, transport, adapter,
    record = async () => {}, runState = {}, campaignId, parentPhase = phase,
    caseBudgetMs = 60000, cleanupBudgetMs = 60000} = environment;
  check([caseBudgetMs, cleanupBudgetMs].every(value => Number.isSafeInteger(value) && value > 0), 'case_finite_budget');
  check(['steady', 'before_restart', 'after_restart'].includes(phase)
    && Number.isSafeInteger(iteration) && iteration > 0 && ['a', 'b'].includes(am), 'case_invocation');
  check(/@sha256:[0-9a-f]{64}$/.test(image ?? '') && /^[0-9a-f]{40}$/.test(source ?? ''), 'case_image_binding');
  const result = {schema: 'multi-am-case-result-v1', id: definition.id, family: row.family,
    source, image, campaign_id: campaignId ?? null, phase, parent_phase: parentPhase, iteration, am, invocation: randomUUID(), status: 'RUNNING',
    steps: [], requests: [], eg_requests: [], platform_events: [], external_platform_proofs: [],
    owner_proofs: [], observations: [], cleanup: [], action_timings: []};
  const actionFinishes = new Set();
  const cleanups = [], completed = new Set();
  const sessions = new Set(), abortCloses = [], wrappedFixtures = new Map();
  let activeStep = null, assertions = 0, primaryError, evidenceFailed = false;
  let controller = new AbortController(), deadline = performance.now() + caseBudgetMs, timedOut = false;
  const remaining = () => Math.max(0, Math.floor(deadline - performance.now()));
  const checkBudget = () => {if (controller.signal.aborted || remaining() <= 0)
    throw controller.signal.reason ?? Error('case_budget_exhausted');};
  const timer = setTimeout(() => {
    timedOut = true; controller.abort(Error('case_budget_exhausted'));
    for (const session of sessions) abortCloses.push(Promise.resolve().then(() => session.close()).catch(() => {}));
  }, caseBudgetMs);
  const retain = async event => {
    try { await record(event); }
    catch { evidenceFailed = true; }
  };
  const started = performance.now();
  const router = new NonOwnerRouter({adapter, record: async proof => {
    result.owner_proofs.push(proof); await retain({kind: 'owner_proof', case_id: row.case_id, proof});
  }});
  const ctx = {row, phase, iteration, am, state: {}, runState,
    get signal() {return controller.signal;}, remainingMs: remaining,
    startAction(boundary) {
      check(activeStep !== null && /^[a-z][a-z0-9_]{0,100}$/.test(boundary), 'action_timing_boundary');
      const start = performance.now(); let finished = false;
      const finish = (status = 'PASS') => {
        if (finished) return; finished = true; actionFinishes.delete(finish);
        check(['PASS', 'FAIL'].includes(status), 'action_timing_status');
        result.action_timings.push({boundary, status, elapsed_ms: performance.now() - start});
      };
      actionFinishes.add(finish); return finish;
    },
    async measureAction(boundary, operation) {
      const finish = ctx.startAction(boundary);
      try {const value = await operation(); finish(); return value;}
      catch (error) {finish('FAIL'); throw error;}
    },
    assert(condition, code) {
      check(activeStep !== null, 'assertion_outside_step');
      check(typeof code === 'string' && /^[a-z][a-z0-9_]{0,100}$/.test(code), 'assertion_code');
      assertions += 1; check(condition, code);
    },
    fixture(name) {
      check(definition.fixtures.includes(name), 'undeclared_fixture');
      if (!(name in fixtures)) throw new Blocked('fixture_unavailable_' + name.replaceAll('-', '_'));
      if (name === 'browser' && typeof fixtures[name]?.open === 'function') {
        if (!wrappedFixtures.has(name)) wrappedFixtures.set(name, {...fixtures[name], async open(options) {
          checkBudget();
          const session = await fixtures[name].open({...options, signal: controller.signal});
          sessions.add(session);
          if (controller.signal.aborted) {await session.close(); checkBudget();}
          return session;
        }});
        return wrappedFixtures.get(name);
      }
      return fixtures[name];
    },
    async step(id, operation) {
      checkBudget();
      check(activeStep === null && definition.steps.includes(id) && !completed.has(id), 'step_identity');
      activeStep = id; assertions = 0;
      const start = performance.now(), value = {id, status: 'RUNNING'};
      result.steps.push(value);
      try {
        await operation(); checkBudget(); check(assertions > 0, 'step_without_assertion');
        value.status = 'PASS'; completed.add(id);
      } catch (error) { value.status = error instanceof Blocked ? 'BLOCKED' : 'FAIL'; throw error; }
      finally { value.assertions = assertions; value.elapsed_ms = performance.now() - start; activeStep = null; }
    },
    async observe(label, operation) {
      check(/^[a-z][a-z0-9_]{0,100}$/.test(label), 'observation_label');
      const value = await operation();
      const encoded = JSON.stringify(value);
      check(typeof encoded === 'string' && encoded.length <= 4 * 1024 * 1024, 'observation_bound');
      if (value?.final_database_cleanup === 'PARENT_MUST_CONFIRM_BEFORE_CAMPAIGN_PASS') {
        result.requires_parent_cleanup = true;
      }
      if (value?.schema === 'multi-eg-request-v1') {
        check(validEgReceipt(value.receipt), 'eg_serving_receipt_invalid');
        result.eg_requests.push(value.receipt);
      }
      if (label === 'platform_event_receipt' && externalPlatform(row.case_id)) result.platform_events.push(value);
      if (value?.schema === 'multi-am-external-platform-v1') {
        check(result.platform_events.some(event => validPlatformProof(value, externalPlatform(row.case_id), event)),
          'external_platform_proof_invalid');
        result.external_platform_proofs.push(value);
      }
      const browserEvidence = value?.schema === 'multi-am-browser-evidence-v1';
      const egEvidence = value?.schema === 'multi-am-eg-evidence-v1';
      if (browserEvidence || egEvidence) {
        check(value.secrets_redacted === true && Array.isArray(value.requests) && value.requests.length > 0,
          'request_observation_invalid');
        if (browserEvidence) check(value.real_ui === true
          && /^[a-f0-9]{40}$/.test(value.compiled_source ?? ''), 'browser_observation_invalid');
        if (egEvidence) check(/^[a-f0-9]{64}$/.test(value.event_sha256 ?? '')
          && /^[a-f0-9]{64}$/.test(value.response_sha256 ?? '')
          && value.requests.some(request => request.owner_proof?.kind === 'non_owner'
            && request.owner_proof?.owner !== request.am), 'eg_am_observation_invalid');
        for (const request of value.requests) {
          check(['a', 'b'].includes(request.am) && typeof request.request_id === 'string'
            && request.request_id.trim().length > 0, 'browser_attribution_missing');
          if (egEvidence) check(/^[a-f0-9]{64}$/.test(request.agent_id_sha256 ?? ''), 'eg_agent_join_missing');
          if (request.agent_id_sha256) {
            const proof = request.owner_proof;
            check(proof?.verdict === 'PASS' && proof.agent_sha256 === request.agent_id_sha256
              && proof.request_id === request.request_id && proof.ingress === request.am
              && (proof.owner !== proof.ingress || (egEvidence && proof.kind === 'explicit_ingress') || (proof.kind === 'peer_read'
                && ['GET', 'HEAD'].includes(proof.method))) && Number.isSafeInteger(proof.owner_epoch)
              && proof.owner_epoch > 0, 'browser_non_owner_proof_missing');
            result.owner_proofs.push(proof);
          }
          result.requests.push(request);
        }
      }
      result.observations.push({label, sha256: hash(encoded)});
      await retain({kind: 'observation', case_id: row.case_id, label, value});
      return value;
    },
    defer(label, operation) {
      check(/^[a-z][a-z0-9_]{0,100}$/.test(label) && typeof operation === 'function', 'cleanup_contract');
      cleanups.push({label, operation});
    },
    async request({actor = 'user', am: requestedAm, method, path, body, key,
      allowed = [200], agentId, peerRead = false, timeout = 30000}) {
      checkBudget();
      check(typeof actor === 'string' && /^[a-z][a-z0-9_-]{0,63}$/.test(actor)
        && (requestedAm === undefined || ['a', 'b'].includes(requestedAm)), 'request_actor_ingress');
      check(['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS'].includes(method)
        && typeof path === 'string' && path.startsWith('/') && !path.startsWith('//')
        && !path.includes('://') && !path.split(/[/?]/).includes('..'), 'request_target');
      if (!transport?.request) throw new Blocked('transport_unavailable');
      if (transport.hasActor && !transport.hasActor(actor)) throw new Blocked('actor_unavailable_' + actor.replaceAll('-', '_'));
      const request = {actor, method, path, body, key, allowed, timeout: Math.max(1, Math.min(timeout, remaining()))};
      const dispatch = async ingress => {
        checkBudget();
        const response = await transport.request({...request, am: ingress, signal: controller.signal});
        // Retain original response before attribution can refuse an accepted write.
        await retain({kind: 'response', case_id: row.case_id, request: {...request, am: ingress}, response});
        result.requests.push({...response.timing, am: ingress, method, status: response.status,
          request_id: response.timing?.request_id, elapsed_ms: response.timing?.body_ms});
        if (response.timing?.am !== ingress || typeof response.timing?.request_id !== 'string'
          || !response.timing.request_id.trim()) {
          const error = Error('request_attribution_missing'); error.response = response; throw error;
        }
        if (!allowed.includes(response.status)) {
          const error = Error('request_status_' + response.status); error.response = response; throw error;
        }
        return response;
      };
      if (agentId) {
        if (!adapter?.ownerOf || !adapter?.topology) throw new Blocked('owner_adapter_unavailable');
        if (peerRead) return router.read(agentId, requestedAm ?? am, dispatch, method);
        return requestedAm === undefined ? router.dispatch(agentId, dispatch)
          : router.at(agentId, requestedAm, dispatch, method);
      }
      return dispatch(requestedAm ?? am);
    },
  };
  try {
    const missing = definition.fixtures.filter(name => !(name in fixtures));
    if (missing.length) throw new Blocked('fixture_unavailable_' + missing[0].replaceAll('-', '_'));
    await definition.run(ctx);
    checkBudget();
    check(completed.size === row.required_steps.length
      && row.required_steps.every(step => completed.has(step)), 'required_steps_incomplete');
    result.status = 'PASS';
  } catch (error) {
    primaryError = error;
    if (timedOut) {result.budget_exhausted = true; result.interrupted_error = safeCode(error); error = controller.signal.reason; primaryError = error;}
    result.status = error instanceof Blocked ? 'BLOCKED' : 'FAIL'; result.error_code = safeCode(error);
    await retain({kind: 'primary_failure', case_id: row.case_id, error_code: result.error_code,
      response: error?.response ?? null});
  } finally {
    for (const finish of actionFinishes) finish('FAIL');
    clearTimeout(timer);
    // Join the interrupted operation and its browser closes before cleanup. No
    // Promise.race abandons a live mutation or permits the next case to start.
    await Promise.all(abortCloses);
    controller = new AbortController(); deadline = performance.now() + cleanupBudgetMs;
    const cleanupTimer = setTimeout(() => controller.abort(Error('case_cleanup_budget_exhausted')), cleanupBudgetMs);
    for (const {label, operation} of cleanups.reverse()) {
      const cleanup = {label, status: 'RUNNING'}; result.cleanup.push(cleanup);
      try { await operation(); cleanup.status = 'PASS'; }
      catch (error) {
        cleanup.status = 'FAIL'; cleanup.error_code = safeCode(error); result.status = 'FAIL';
        cleanup.error_codes = errorCodes(error);
        await retain({kind: 'cleanup_failure', case_id: row.case_id, label,
          error_code: cleanup.error_code, error_codes: cleanup.error_codes, response: error?.response ?? null});
      }
    }
    clearTimeout(cleanupTimer);
    // Browser captures are deliberately flushed by deferred cleanup, after the
    // final UI action. Validate their receipts only after that flush has joined.
    if (result.status === 'PASS') {
      try {
        check(result.requests.length > 0 || result.observations.length > 0, 'actual_observation_required');
        if (definition.scope === 'agent') check(result.owner_proofs.some(proof =>
          proof.verdict === 'PASS' && proof.owner !== proof.ingress), 'non_owner_evidence_required');
      } catch (error) {
        primaryError = error; result.status = 'FAIL'; result.error_code = safeCode(error);
        await retain({kind: 'primary_failure', case_id: row.case_id, error_code: result.error_code, response: null});
      }
    }
    result.elapsed_ms = performance.now() - started;
    if (evidenceFailed) { result.status = 'FAIL'; result.evidence_error = 'private_evidence_write_failed'; }
    result.functional_status = result.status;
    if (result.status === 'PASS' && !platformComplete(row.case_id, result)) {
      result.status = 'BLOCKED'; result.error_code = 'actual_platform_proof_missing';
    }
    result.performance = latencyVerdict(row, result);
    if (result.status === 'PASS' && ['FAIL', 'MISSING'].includes(result.performance)) {
      result.status = result.performance === 'FAIL' ? 'FAIL' : 'BLOCKED';
      result.error_code = result.performance === 'FAIL' ? 'action_latency_target_exceeded' : 'action_latency_evidence_missing';
    }
    result.primary_failure = primaryError ? safeCode(primaryError) : null;
    await retain({kind: 'case_result', result});
    if (evidenceFailed) { result.status = 'FAIL'; result.evidence_error = 'private_evidence_write_failed'; }
  }
  return result;
}
