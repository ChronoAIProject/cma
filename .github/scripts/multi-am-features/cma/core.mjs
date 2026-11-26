import {createHash} from 'node:crypto';
import {performance} from 'node:perf_hooks';
import {requestPacer} from '../pacing.mjs';

export const hash = x => createHash('sha256').update(String(x)).digest('hex');
export const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
export function check(value, code) { if (!value) throw new Error(code); }
export const id = x => typeof x === 'string' && /^[-a-zA-Z0-9_]{1,160}$/.test(x);
export const other = am => am === 'a' ? 'b' : 'a';
export function safeError(error) { return /^[a-zA-Z0-9_]+$/.test(error?.message ?? '') ? error.message : 'transport_or_adapter_error'; }
export const terminal = value => ['completed', 'failed', 'incomplete', 'cancelled'].includes(value.status);
export const lineage = (record, detail) => detail?.id === record.agentId && detail.sandbox?.id === record.sandboxId
  && [detail.title, detail.owner_title, detail.worktree?.name, detail.sandbox?.name]
    .some(name => name === record.name || (record.writerTitle && name === record.writerTitle.title));
export function runtimeBindings(value) {
  check(value?.runtime_images?.base, 'base_runtime_image_required');
  const bindings = {};
  for (const [family, image] of Object.entries(value.runtime_images)) {
    if (!image) continue;
    check(typeof image === 'string' && /@sha256:[a-f0-9]{64}$/.test(image), 'exact_runtime_image_required');
    const source = value.runtime_sources?.[family];
    check(typeof source === 'string' && /^[a-f0-9]{40}$/.test(source), 'exact_runtime_family_source_required');
    bindings[family] = {image, source};
  }
  return bindings;
}
export function assertRuntimeBindings(expected, value) {
  const observed = runtimeBindings(value);
  for (const [family, binding] of Object.entries(expected)) {
    check(observed[family]?.image === binding.image, 'runtime_family_image_mismatch');
    check(observed[family]?.source === binding.source, 'runtime_family_source_mismatch');
  }
  return observed;
}
export async function poll(read, accept, {timeout = 180000, interval = 1000, code = 'poll_timeout'} = {}) {
  const end = performance.now() + timeout;
  do { const value = await read(); if (accept(value)) return value; if (performance.now() >= end) throw new Error(code); await wait(interval); } while (true);
}
export async function awaitInitialIdle(read, {timeout = 90000, interval = 1000} = {}) {
  return poll(read, value => {
    check(['ready', 'working'].includes(value?.status), 'open_not_ready');
    const message = value.first_message;
    if (!message) {check(value.status === 'ready', 'open_working_without_first_message'); return true;}
    check(['setup', 'task'].includes(message.purpose), 'open_first_message_purpose_unknown');
    check(['queued', 'dispatch_fenced', 'write_confirmed', 'execution_observed', 'completed'].includes(message.state), 'open_first_message_failed');
    return value.status === 'ready' && message.state === 'completed';
  }, {timeout, interval, code: 'open_initial_message_timeout'});
}
export function assertCreated(event, agentId) {
  check(event?.type === 'response.created' && event.sequence_number === 0 && id(event.response?.id)
    && event.response.conversation?.id === agentId, 'response_created_identity_mismatch');
  return event.response.id;
}
export function assertResponse(value, agentId, responseId) {
  check(value?.id === responseId && value.conversation?.id === agentId, 'response_identity_mismatch');
  return value;
}
export function assertOwner(owner, agentId) {
  check(['a', 'b'].includes(owner?.am) && Number.isSafeInteger(owner.epoch) && owner.epoch > 0,
    'owner_identity_missing');
  check(owner.agent_id === agentId || owner.agent_sha256 === hash(agentId), 'owner_agent_mismatch');
  check(typeof owner.pod_uid === 'string' && owner.pod_uid.length > 0, 'owner_pod_missing');
  return owner;
}
export function assertRecovery(before, after, agentId) {
  assertOwner(before, agentId); assertOwner(after, agentId);
  check(after.am !== before.am && after.pod_uid !== before.pod_uid && after.epoch > before.epoch,
    'owner_recovery_not_proven');
}
export function assertOracle(value, caseId, completed) {
  check(value?.case_id === caseId && value.request_count === 1, 'provider_request_count_mismatch');
  check(value.completed_effect_count === completed && value.active_streams === 0, 'provider_completion_count_mismatch');
}
export async function observeServingDuringRestart(restart, read, {interval = 500} = {}) {
  const started = performance.now();
  let finished = false, restartError, readError, servingSamples = 0;
  const pending = Promise.resolve().then(restart).then(
    value => {finished = true; return value;},
    error => {finished = true; restartError = error; return null;});
  try {
    do {
      const beganWhileStarting = !finished;
      await read();
      if (beganWhileStarting && !finished) servingSamples += 1;
      if (!finished) await wait(interval);
    } while (!finished);
  } catch (error) {readError = error;}
  const restored = await pending;
  if (restartError) throw restartError;
  if (readError) throw readError;
  check(servingSamples > 0, 'survivor_serving_during_restart_unobserved');
  return {restored, servingSamples, elapsed_ms: performance.now()-started};
}
export const TWO_AM_HOLD_MS = 600000;
export async function observeTwoAmServingHold({topology, read, sample, expectedImage},
  {now = () => performance.now(), sleep = wait} = {}) {
  const interval = 5000, maxGap = 15000;
  let baseline, firstSuccess, lastSuccess, samples = 0, maxObservedGap = 0;
  while (true) {
    const began = now();
    const current = await topology();
    check(current?.coordination_enabled === true && current.shared_database === true,
      'hold_coordination_or_database_changed');
    const identities = {};
    for (const am of ['a', 'b']) {
      const member = current.ams?.[am];
      check(member?.ready === true && member.image === expectedImage, 'hold_am_not_ready_or_image_changed');
      check(member.draining === false, 'hold_member_draining_or_unbound');
      for (const key of ['pod_uid', 'instance_id', 'container_id', 'started_at']) {
        check(typeof member[key] === 'string' && member[key].length > 0, 'hold_process_identity_unbound');
      }
      check(Number.isFinite(Date.parse(member.started_at)), 'hold_process_start_unbound');
      check(member.restarts === 0, 'hold_am_restarted');
      identities[am] = Object.fromEntries(['pod_uid', 'instance_id', 'container_id', 'started_at'].map(key => [key, member[key]]));
      if (baseline) check(Object.keys(identities[am]).every(key => identities[am][key] === baseline[am][key]),
        'hold_am_process_changed');
    }
    check(identities.a.pod_uid !== identities.b.pod_uid && identities.a.instance_id !== identities.b.instance_id,
      'hold_two_distinct_processes_required');
    baseline ??= identities;
    for (const am of ['a', 'b']) await read(am);
    const completed = now();
    check(completed - began <= maxGap, 'hold_sample_too_slow');
    if (lastSuccess !== undefined) {
      const gap = completed - lastSuccess;
      check(gap <= maxGap, 'hold_serving_observation_gap');
      maxObservedGap = Math.max(maxObservedGap, gap);
    }
    firstSuccess ??= completed;
    lastSuccess = completed; samples += 1;
    const progress = {samples, elapsed_ms: completed-firstSuccess, max_observed_gap_ms:maxObservedGap,
      required_duration_ms:TWO_AM_HOLD_MS, sampling_interval_ms:interval, max_allowed_gap_ms:maxGap};
    await sample(progress);
    // Both AMs must complete the final useful-read pair at or after ten full
    // minutes from the first pair. A timer or a final Ready snapshot is insufficient.
    if (progress.elapsed_ms >= TWO_AM_HOLD_MS) return {...progress, processes:baseline};
    await sleep(Math.max(0, interval-(now()-began)));
  }
}
export function normalizeConfig(raw) {
  const origins = raw.am_origins ?? raw.ams;
  const ams = Object.fromEntries(['a', 'b'].map(k => [k, typeof origins?.[k] === 'string' ? origins[k] : origins?.[k]?.origin]));
  for (const value of Object.values(ams)) {
    const u = new URL(value); check(['127.0.0.1', 'localhost', '[::1]'].includes(u.hostname)
      && u.protocol === 'http:' && !u.username && !u.password, 'nonlocal_am_origin');
  }
  check(ams.a !== ams.b, 'two_distinct_am_origins_required');
  const bearer = raw.bearer ?? raw.auth?.bearer;
  check(typeof bearer === 'string' && bearer.length > 0, 'fixture_bearer_missing');
  return {...raw, ams, bearer, owner_id: raw.owner_id ?? raw.auth?.owner_id,
    expected_api_image: raw.expected_api_image ?? raw.api?.image,
    expected_api_source: raw.expected_api_source ?? raw.api?.source,
    api_release: raw.api_release ?? raw.api?.release,
    runtime_images: raw.runtime_images ?? raw.runtime?.images ?? raw.runtime,
    runtime_sources: raw.runtime_sources ?? raw.runtime?.sources,
    output_dir: raw.output_dir ?? raw.output?.dir ?? raw.output,
    public_origin: raw.public_origin ?? raw.auth?.origin ?? 'https://native-am.local.test',
    host: raw.host ?? raw.auth?.host ?? 'native-am.local.test'};
}

export class API {
  constructor(config, observe = () => {}) {
    this.config = config; this.observe = observe; this.streams = new Set();
    this.routedAgents = new Set(); this.responseAgents = new Map(); this.routingEnabled = true;
    this.commandAgents = new Map(); this.sandboxAgents = new Map(); this.admissionAgents = new Map();
  }
  bindSandbox(sandbox, agent) {
    check(id(sandbox) && id(agent), 'sandbox_agent_binding_invalid');
    check(!this.sandboxAgents.has(sandbox) || this.sandboxAgents.get(sandbox) === agent,
      'sandbox_agent_binding_changed');
    this.sandboxAgents.set(sandbox, agent); this.routedAgents.add(agent);
  }
  bindAdmission(key, agent) {
    check(id(key) && this.routedAgents.has(agent), 'admission_agent_binding_invalid');
    check(!this.admissionAgents.has(key) || this.admissionAgents.get(key) === agent,
      'admission_agent_binding_changed');
    this.admissionAgents.set(key, agent);
  }
  headers(extra = {}) { return {Authorization: 'Bearer ' + this.config.bearer, Host: this.config.host, Origin: this.config.public_origin, ...extra}; }
  url(am, path) { check(['a', 'b'].includes(am) && path.startsWith('/api/'), 'api_target_invalid'); return new URL(path, this.config.ams[am]); }
  async request(am, method, path, options = {}) {
    await requestPacer.wait(this.config.bearer, method);
    const agent = path.match(/^\/api\/v[12]\/agents\/([^/?]+)/)?.[1]
      ?? this.responseAgents.get(path.match(/^\/api\/v2\/responses\/([^/?]+)/)?.[1])
      ?? this.commandAgents.get(path.match(/^\/api\/v2\/commands\/([^/?]+)/)?.[1])
      ?? this.sandboxAgents.get(path.match(/^\/api\/v1\/admin\/sandboxes\/([^/?]+)/)?.[1])
      ?? (method === 'POST' && path === '/api/v1/agents' ? this.admissionAgents.get(options.key) : undefined);
    const response = this.router && this.routingEnabled && this.routedAgents.has(agent)
      ? await this.router.dispatch(agent, ingress => this.rawRequest(ingress, method, path, options))
      : await this.rawRequest(am, method, path, options);
    const command = response.body?.control?.receipt?.command_id ?? response.body?.receipt?.command_id;
    if (agent && id(command)) {
      check(!this.commandAgents.has(command) || this.commandAgents.get(command) === agent,
        'command_agent_binding_changed');
      this.commandAgents.set(command, agent);
    }
    return response;
  }
  async rawRequest(am, method, path, {body, key, allowed = [200], timeout = 30000, documentShareToken, prefer} = {}) {
    const start = performance.now();
    const headers = this.headers({...(body === undefined ? {} : {'Content-Type': 'application/json'}),
      ...(key ? {'Idempotency-Key': key} : {}),
      ...(documentShareToken ? {'X-CMA-Document-Share-Token': documentShareToken} : {}),
      ...(prefer ? {Prefer: prefer} : {})});
    const result = await fetch(this.url(am, path), {method, headers,
      body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(timeout), redirect: 'error'});
    const headerMs = performance.now() - start;
    const text = await result.text(); check(text.length < 4 * 1024 * 1024, 'api_body_limit');
    let value = null; if (text) { try { value = JSON.parse(text); } catch { throw new Error('api_json_invalid'); } }
    const observation = {am, method, route: path.split('?')[0].replace(/(\/(?:agents|sandboxes|agent-lifecycle|responses|commands))\/[^/]+/g, '$1/{id}'),
      status: result.status, header_ms: headerMs, body_ms: performance.now() - start,
      request_id: result.headers.get('x-request-id')};
    this.observe(observation);
    const response = {status: result.status, body: value, timing: observation,
      retry_after: result.headers.get('retry-after')};
    if (!allowed.includes(result.status)) {
      const error = new Error('api_status_' + result.status);
      // Caller may retain this privately; public errors contain only the fixed code.
      error.response = response; throw error;
    }
    return response;
  }
  async send(am, agentId, input, key, protocol = 'agent-responses') {
    await requestPacer.wait(this.config.bearer, 'POST');
    const value = this.router && this.routingEnabled
      ? await this.router.dispatch(agentId, ingress => this.rawSend(ingress, agentId, input, key, protocol))
      : await this.rawSend(am, agentId, input, key, protocol);
    this.responseAgents.set(value.responseId, agentId);
    return value;
  }
  async rawSend(am, agentId, input, key, protocol = 'agent-responses') {
    const agui = protocol === 'agui';
    const root = protocol === 'root-responses';
    check(agui || root || protocol === 'agent-responses', 'unknown_conversation_ingress');
    const route = agui ? '/api/v1/agui/run' : root ? '/api/v2/responses' : '/api/v2/agents/{id}/responses';
    const payload = agui ? {threadId: agentId, messages: [{role: 'user', content: input}]}
      : {input, stream: true, ...(root ? {conversation: agentId} : {})};
    const abort = new AbortController(); const timer = setTimeout(() => abort.abort(), 240000);
    const handle = {ingress_am: am, abort: () => abort.abort(), error: null, events: [], done: null}; this.streams.add(handle);
    const start = performance.now();
    let settle, fail;
    const created = new Promise((r, j) => {settle = r; fail = j;});
    handle.done = (async () => {
      let seen = false;
      try {
        const response = await fetch(this.url(am, route.replace('{id}', agentId)), {
          method: 'POST', headers: this.headers({'Content-Type': 'application/json', 'Idempotency-Key': key}),
          body: JSON.stringify(payload), signal: abort.signal, redirect: 'error'});
        handle.timing = {am, method: 'POST', route,
          status: response.status, header_ms: performance.now() - start,
          request_id: response.headers.get('x-request-id')};
        this.observe(handle.timing);
        check(response.status === 200 || response.status === 202, 'send_status_' + response.status);
        check((response.headers.get('content-type') ?? '').includes('text/event-stream'), 'send_not_sse');
        handle.commandId = response.headers.get('x-cma-command-id');
        const responseHeader = response.headers.get('x-cma-response-id');
        const decoder = new TextDecoder(); let buffer = ''; let bytes = 0;
        for await (const chunk of response.body) {
          bytes += chunk.length; check(bytes < 8 * 1024 * 1024, 'sse_byte_limit'); buffer += decoder.decode(chunk, {stream: true});
          buffer = buffer.replace(/\r\n/g, '\n');
          let end;
          while ((end = buffer.indexOf('\n\n')) >= 0) {
            const frame = buffer.slice(0, end); buffer = buffer.slice(end + 2);
            const data = frame.split('\n').filter(x => x.startsWith('data:')).map(x => x.slice(5).trimStart()).join('\n');
            if (!data || data === '[DONE]') continue;
            const event = JSON.parse(data);
            if (event.type === (agui ? 'RUN_STARTED' : 'response.created')) {
              check(!seen, 'duplicate_created_frame');
              if (agui) {
                check(event.threadId === agentId && typeof event.runId === 'string'
                  && /^[1-9][0-9]*$/.test(event.runId) && Number.isSafeInteger(Number(event.runId))
                  && id(responseHeader) && id(handle.commandId), 'agui_run_receipt_invalid');
                handle.runId = event.runId;
              }
              const responseId = agui ? responseHeader : assertCreated(event, agentId);
              if (root) check(id(handle.commandId) && responseHeader === responseId, 'root_response_receipt_invalid');
              seen = true;
              handle.responseId = responseId; handle.first_event_ms = performance.now() - start; settle(handle);
            }
            if (agui && event.type === 'RUN_FINISHED') {
              check(seen && event.threadId === agentId && event.runId === handle.runId,
                'agui_completion_receipt_invalid');
              handle.runOutcome = event.outcome?.type;
            }
            handle.events.push({type: event.type, sequence_number: event.sequence_number});
          }
        }
        check(seen, agui ? 'missing_agui_run_started' : 'missing_response_created');
      } catch (error) { handle.error = error.message; if (!seen) fail(error); }
      finally { clearTimeout(timer); this.streams.delete(handle); }
    })();
    return created;
  }
  async close() { const streams = [...this.streams]; for (const stream of streams) stream.abort(); await Promise.all(streams.map(x => x.done)); }
}
