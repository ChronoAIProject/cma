import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual} from 'node:util';
import {check, hash, waitFor} from '../scale-tests/lib/checks.mjs';
import {LocalHttp} from '../scale-tests/lib/http.mjs';
import {guardedFetch} from '../scale-tests/lib/safety.mjs';
import {campaign, sameHome, validateLocalOrigins} from './engine.mjs';
import {readCampaignTranscript} from './campaign-transcript.mjs';
import {isNativeSuspended} from './lifecycle.mjs';
import {createNativeReader} from './native-reader.mjs';

const terminalFailure = new Set(['failed', 'cancelled', 'incomplete']);
const identifier = value => typeof value === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(value);
const positive = value => Number.isSafeInteger(value) && value > 0;
const digest = value => hash(JSON.stringify(value));
const key = () => randomUUID();
const running = snapshot => snapshot.operating_mode === 'Running'
  && snapshot.pods.some(pod => pod.phase === 'Running' && pod.deleting === false);

// The fixture owns local custody, runtime scheduling and the synthetic provider.
// Route calls and the independent native reader establish the campaign evidence.
export async function runCmaCampaign(options, {count = 100, record, now} = {}) {
  const adapter = createCmaAdapter({...options, now: now ?? options.now});
  check(typeof record === 'function', 'private_recorder_required');
  let authorized = false;
  let finalPacket;
  let result;
  try {
    result = await campaign({...adapter, async validate() {
      await adapter.validate();
      authorized = true;
    }}, {count, now, async record(packet) {
      if (packet.verdict === 'PASS') {
        finalPacket = packet;
      } else {
        await record(packet);
      }
    }});
  } finally {
    if (authorized) {
      try {
        await options.fixture.cleanup();
      } catch {
        if (finalPacket) {
          await record({...finalPacket, summary: {...finalPacket.summary, verdict: 'FAIL'},
            verdict: 'FAIL', error_code: 'cleanup_failed'});
        }
        check(false, 'cleanup_failed');
      }
    }
  }
  if (finalPacket) await record(finalPacket);
  return result;
}

export function createCmaAdapter({origin, nativeOrigin, ownerBearer, adminBearer, nativeBearer,
  fixture, intervalMs = 250, requestMs = 10000, resumeMs = 30000,
  idleMs = 930000, turnMs = 120000, pauseAfterSeconds = 900, now = () => performance.now(), fetchImpl} = {}) {
  const origins = {ams: {owner: origin}, control_origin: nativeOrigin};
  validateLocalOrigins(origins);
  check([ownerBearer, adminBearer, nativeBearer].every(value => typeof value === 'string' && value.length),
    'campaign_credentials_required');
  check(ownerBearer !== adminBearer, 'campaign_distinct_callers_required');
  check([intervalMs, requestMs, resumeMs, idleMs, turnMs].every(positive)
    && requestMs <= 30000 && resumeMs <= 120000 && idleMs <= 86430000 && turnMs <= 86430000,
  'campaign_deadline_invalid');
  check(positive(pauseAfterSeconds) && pauseAfterSeconds >= 60 && pauseAfterSeconds <= 86400,
    'campaign_inactivity_threshold_invalid');
  check(['validate', 'prepare', 'idleOpportunity', 'waitForInactivity', 'reconciliationCount', 'holdFirstTurn', 'cleanup']
    .every(method => typeof fixture?.[method] === 'function'), 'campaign_fixture_incomplete');
  const owner = new LocalHttp(origin, {bearer: ownerBearer, timeoutMs: requestMs, fetchImpl});
  const admin = new LocalHttp(origin, {bearer: adminBearer, timeoutMs: requestMs, fetchImpl});
  const fetchLocal = guardedFetch(fetchImpl);
  const submitted = [];
  let native;
  const agentRoute = (baseline, suffix = '') => `/api/v1/agents/${baseline.agentId}${suffix}`;
  const sandboxRoute = baseline => `/api/v1/sandboxes/${baseline.sandboxId}`;
  const responseRoute = turn => `/api/v2/responses/${turn.responseId}`;
  const wait = (read, accept, timeoutMs) => waitFor(
    remaining => read(AbortSignal.timeout(Math.max(1, Math.ceil(remaining)))),
    accept, {timeoutMs, intervalMs},
  );
  const call = async (client, path, route, options = {}) => {
    try {
      return (await client.request(path, {route, ...options})).value;
    } finally {
      client.samples.length = 0;
    }
  };
  const sandbox = async (baseline, signal) => {
    const value = await call(owner, sandboxRoute(baseline), '/api/v1/sandboxes/:id', {signal});
    check(value?.id === baseline.sandboxId, 'campaign_sandbox_identity_changed');
    return value;
  };
  const agent = (baseline, signal) => call(owner, agentRoute(baseline), '/api/v1/agents/:id', {signal});
  const conversation = baseline => call(owner, `/api/v2/conversations/${baseline.agentId}`,
    '/api/v2/conversations/:id');
  const response = (turn, signal) => call(owner, responseRoute(turn), '/api/v2/responses/:id', {signal});
  const matchesResponse = (baseline, turn, value) => {
    check(value?.object === 'response' && value.id === turn.responseId
      && value.conversation?.id === baseline.conversationId, 'campaign_response_identity_changed');
    check(!terminalFailure.has(value.status), 'campaign_response_failed');
    return value;
  };
  const transcript = baseline => {
    const signal = AbortSignal.timeout(turnMs);
    return readCampaignTranscript(baseline.agentId, query => call(owner,
      `${agentRoute(baseline, '/transcript')}?${query}`, '/api/v1/agents/:id/transcript', {signal}));
  };
  const file = async baseline => {
    const query = new URLSearchParams({path: baseline.filePath});
    const value = await call(owner, `${agentRoute(baseline, '/files/content')}?${query}`,
      '/api/v1/agents/:id/files/content');
    check(typeof value?.content_base64 === 'string' && value.content_base64.length <= 16384,
      'campaign_file_invalid');
    const bytes = Buffer.from(value.content_base64, 'base64');
    check(bytes.length > 0 && value.content_base64 === bytes.toString('base64')
      && value.size_bytes === bytes.length && value.sha256 === hash(bytes),
      'campaign_file_invalid');
    return value.sha256;
  };

  async function receipt(baseline, turn) {
    const status = await call(owner, `/api/v2/commands/${turn.commandId}`, '/api/v2/commands/:id');
    const lookup = await call(owner, `/api/v2/agents/${baseline.agentId}/responses/receipt`,
      '/api/v2/agents/:id/responses/receipt', {method: 'POST', key: turn.key, body: turn.body});
    const value = status?.receipt;
    check(status?.object === 'cma.agent.command' && lookup?.object === status.object
      && value?.agent_id === baseline.agentId
      && value.command_id === turn.commandId && value.response_id === turn.responseId
      && positive(value.agent_order) && status.status === 'completed' && positive(status.turn_sequence)
      && lookup.status === status.status && lookup.turn_sequence === status.turn_sequence
      && isDeepStrictEqual(lookup.receipt, value), 'campaign_receipt_changed');
    return {receipt: digest(value), sequence: status.turn_sequence};
  }

  async function verify(baseline) {
    const currentConversation = await conversation(baseline);
    check(digest(currentConversation) === baseline.conversationHash, 'campaign_conversation_changed');
    sameHome(baseline.home, await native());
    check(await file(baseline) === baseline.fileHash, 'campaign_file_changed');
    const turns = await transcript(baseline);
    check(turns.length === baseline.history.length + submitted.length, 'campaign_transcript_count_changed');
    for (const [index, expected] of baseline.history.entries()) {
      check(digest(turns[index]) === expected, 'campaign_original_history_changed');
    }
    for (const [index, turn] of submitted.entries()) {
      const saved = turns[baseline.history.length + index];
      check(saved.turn_sequence === turn.sequence && saved.owner_message === turn.body.input
        && digest(saved) === turn.transcriptHash, 'campaign_retained_turn_changed');
    }
  }

  async function verifyProtectedInterval(sample) {
    await sample();
    const started = now();
    const thresholdMs = pauseAfterSeconds * 1000;
    await fixture.waitForInactivity(thresholdMs);
    check(Number.isFinite(started) && now() - started >= thresholdMs, 'campaign_inactivity_interval_short');
    const before = await fixture.reconciliationCount();
    await fixture.idleOpportunity();
    const after = await fixture.reconciliationCount();
    check(Number.isSafeInteger(before) && before >= 0 && positive(after) && after > before,
      'campaign_reconciliation_unobserved');
    await sample();
    const elapsed = now() - started;
    check(Number.isFinite(elapsed) && elapsed >= thresholdMs, 'campaign_inactivity_interval_short');
    return elapsed;
  }

  function settledOutput(result, turn) {
    const output = result.output;
    check(Array.isArray(output) && output.length === 1, 'campaign_response_output_invalid');
    const [message] = output;
    check(message?.type === 'message' && message.role === 'assistant' && message.status === 'completed'
      && Array.isArray(message.content) && message.content.length === 1
      && message.content[0]?.type === 'output_text' && message.content[0].text === turn.expected,
    'campaign_response_output_invalid');
    return digest(output);
  }

  return {
    origins,
    validate: () => fixture.validate(origins),
    async prepare() {
      const {settings} = await call(admin, '/api/v1/admin/settings', '/api/v1/admin/settings');
      check(settings?.inactivity?.auto_pause_enabled === true
        && settings.inactivity.pause_after_seconds === pauseAfterSeconds, 'campaign_inactivity_policy_invalid');
      const supplied = await fixture.prepare();
      check(identifier(supplied.agentId) && identifier(supplied.sandboxId)
        && typeof supplied.filePath === 'string' && supplied.filePath.length > 0 && supplied.filePath.length <= 1024
        && supplied.binding?.sandbox_id === supplied.sandboxId, 'campaign_fixture_binding_invalid');
      native = createNativeReader({origin: nativeOrigin, binding: supplied.binding,
        bearer: nativeBearer, timeoutMs: requestMs, fetchImpl});
      const baseline = {agentId: supplied.agentId, sandboxId: supplied.sandboxId, filePath: supplied.filePath};
      const current = await agent(baseline);
      check(current?.id === baseline.agentId && current.sandbox?.id === baseline.sandboxId
        && current.status === 'ready' && current.sandbox.status === 'active', 'campaign_agent_not_idle');
      const initial = await conversation(baseline);
      check(initial?.object === 'conversation' && initial.id === baseline.agentId, 'campaign_conversation_missing');
      const home = await native();
      check(running(home), 'campaign_original_home_unproved');
      const history = await transcript(baseline);
      check(history.length > 0 && history.length <= 20, 'campaign_original_history_missing');
      return {...baseline, conversationId: initial.id, conversationHash: digest(initial),
        home, history: history.map(digest), fileHash: await file(baseline)};
    },
    async alwaysOnException(baseline) {
      const current = await sandbox(baseline);
      check(current?.id === baseline.sandboxId && positive(current.revision) && current.always_on !== true,
        'campaign_original_policy_invalid');
      const body = {expected_revision: current.revision, always_on: true};
      const denied = await call(owner, `${sandboxRoute(baseline)}/always-on`, '/api/v1/sandboxes/:id/always-on',
        {method: 'POST', key: key(), body, expected: [403]});
      check(denied?.code === 'always_on_approval_required', 'campaign_owner_approval_unproved');
      const adminPath = `/api/v1/admin/sandboxes/${baseline.sandboxId}/always-on`;
      const options = value => ({method: 'POST', key: key(), body: value});
      const change = value => call(admin, adminPath, '/api/v1/admin/sandboxes/:id/always-on', options(value));
      const enabled = await change(body);
      check(enabled?.always_on === true && positive(enabled.revision), 'campaign_admin_approval_unproved');
      await verifyProtectedInterval(async () => {
        const observed = await native();
        check((await sandbox(baseline)).status === 'active' && running(observed), 'campaign_exception_did_not_hold');
        sameHome(baseline.home, observed);
      });
      const disabled = await change({always_on: false, expected_revision: enabled.revision});
      check(disabled?.always_on === false, 'campaign_exception_restore_failed');
    },
    async awaitAutomaticPause(baseline) {
      await fixture.idleOpportunity();
      await wait(signal => sandbox(baseline, signal), value => value?.status === 'paused', idleMs);
      const observed = await wait(signal => native({signal}), isNativeSuspended, resumeMs);
      sameHome(baseline.home, observed);
      return observed;
    },
    async wakeWithMessage(baseline, index) {
      const expected = `CMA-RETAINED-CYCLE-${index}`;
      const body = {input: `Reply with exactly ${expected}.`, stream: true};
      const turn = {key: key(), body, expected};
      const result = await fetchLocal(`${owner.origin}/api/v2/agents/${baseline.agentId}/responses`, {
        method: 'POST',
        headers: {Authorization: `Bearer ${ownerBearer}`, 'Content-Type': 'application/json',
          Accept: 'text/event-stream', 'Idempotency-Key': turn.key},
        body: JSON.stringify(body), signal: AbortSignal.timeout(requestMs),
      });
      try {
        turn.responseId = result.headers.get('x-cma-response-id');
        turn.commandId = result.headers.get('x-cma-command-id');
        check(result.status === 200 && result.headers.get('content-type')?.startsWith('text/event-stream')
          && identifier(turn.responseId) && identifier(turn.commandId), 'campaign_input_receipt_missing');
      } finally {
        await result.body?.cancel();
      }
      await wait(async signal => {
        const current = matchesResponse(baseline, turn, await response(turn, signal));
        const observed = await native({signal});
        return ['in_progress', 'completed'].includes(current.status)
          && running(observed);
      }, Boolean, resumeMs);
      return turn;
    },
    home: () => native(),
    async holdActiveTurn(baseline, turn) {
      let elapsed;
      await fixture.holdFirstTurn(async () => {
        elapsed = await verifyProtectedInterval(async () => {
          const value = matchesResponse(baseline, turn, await response(turn));
          check(value.status === 'in_progress' && (await sandbox(baseline)).status === 'active',
            'campaign_active_turn_interrupted');
          const state = await native();
          check(running(state), 'campaign_active_runtime_gone');
          sameHome(baseline.home, state);
        });
      });
      check(elapsed !== undefined, 'campaign_active_turn_samples_missing');
      return {interrupted: false, samples: 2, hold_ms: elapsed};
    },
    async completeTurn(baseline, turn) {
      const result = await wait(async signal => matchesResponse(baseline, turn, await response(turn, signal)),
        value => value.status === 'completed', turnMs);
      const responseHash = settledOutput(result, turn);
      const settled = await receipt(baseline, turn);
      const turns = await transcript(baseline);
      check(turns.length === baseline.history.length + submitted.length + 1, 'campaign_duplicate_or_missing_turn');
      const newest = turns.at(-1);
      const userMessages = newest.messages.filter(message => message.role === 'user');
      check(newest.turn_sequence === settled.sequence && newest.owner_message === turn.body.input
        && userMessages.length === 1 && userMessages[0].content === turn.body.input
        && newest.outcome?.type === 'completed' && newest.agent_messages.join('') === turn.expected,
      'campaign_wake_turn_missing');
      submitted.push({...turn, ...settled, responseHash, transcriptHash: digest(newest)});
      return {receipt_sha256: settled.receipt, response_sha256: responseHash};
    },
    async verifyRetention(baseline) {
      await verify(baseline);
    },
    async verifyAllReceipts(baseline) {
      await verify(baseline);
      for (const turn of submitted) {
        const saved = await receipt(baseline, turn);
        check(saved.receipt === turn.receipt && saved.sequence === turn.sequence,
          'campaign_original_receipt_changed');
        const value = matchesResponse(baseline, turn, await response(turn));
        check(value.status === 'completed' && settledOutput(value, turn) === turn.responseHash,
          'campaign_original_response_changed');
      }
    },
  };
}
