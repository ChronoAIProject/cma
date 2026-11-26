import test from 'node:test';
import assert from 'node:assert/strict';
import {configurationCases, heldConfigurationCases, assertWriterTitle,
  verifyLifecycleReplay} from './agent_configuration.mjs';

function fixture({falseAck = false, lostAck = false, busy = false, badRefusal = false,
  mutateOnRefusal = false, effect = [3], staleNativePrompt = false, staleNativeEffort = false} = {}) {
  let preferences = {title: 'Original', system_prompt: 'Original instructions',
    prompt_pending: false, prompt_supported: true, revision: 1};
  let settings = {model: 'fixture-model', reasoning_effort: 'high',
    permission_mode: 'autonomous', pending: false};
  const original = structuredClone({preferences, settings}), calls = [], rows = [], failures = [];
  const replies = new Map();
  const record = {agentId: 'agent', name: 'Original'};
  const probes = [];
  const api = {request: async (am, method, path, options = {}) => {
    const {body, allowed = [200], key} = options;
    calls.push({am, method, path, ...structuredClone(options)});
    const prompt = path.endsWith('/preferences');
    if (method === 'GET') return {body: structuredClone(prompt ? preferences : settings)};
    if (replies.has(key)) return structuredClone(replies.get(key));
    if (busy && !body.title) {
      if (mutateOnRefusal) preferences.system_prompt = 'corrupted by refused write';
      return {status: 409, body: {code: badRefusal ? 'record_revision_changed' : 'agent_not_ready'}};
    }
    if (prompt && body.expected_revision !== preferences.revision) {
      assert.deepEqual(allowed, [409]);
      return {status: 409, body: {code: 'conflict', settings_update_reason: 'stale_revision'}};
    }
    if (prompt) {
      if (!falseAck) preferences = {...preferences, revision: preferences.revision + 1,
        ...(body.title === undefined ? {system_prompt: body.system_prompt} : {title: body.title})};
      if (lostAck && preferences.revision === 2) throw Error('lost_ack');
    } else settings = {...settings, ...body};
    const response = {status: 200, body: structuredClone(prompt ? preferences : settings)};
    replies.set(key, response);
    return response;
  }};
  let armed;
  api.send = async (_am, agentId, input, key) => {
    assert.equal(agentId, record.agentId);
    assert.equal(key, armed.caseId);
    assert.ok(input.includes(armed.marker));
    assert.ok(!armed.configuration.instructions || !input.includes(armed.configuration.instructions));
    return {responseId: 'response', error: null, events: [{type: 'response.completed'}],
      abort() {}, done: Promise.resolve()};
  };
  api.rawRequest = async (_am, method, path) => {
    assert.equal(method, 'GET');
    if (path === '/api/v1/agents/' + record.agentId) return {body: {id: record.agentId, status: 'ready'}};
    assert.equal(path, '/api/v2/responses/response');
    return {body: {id: 'response', status: 'completed', conversation: {id: record.agentId},
      output: [{text: 'WS4 complete ' + armed.marker}]}};
  };
  const runCase = async (name, operation) => {
    const row = {case: name, verdict: 'RUNNING'}; rows.push(row);
    try { await operation(); row.verdict = 'PASS'; }
    catch (error) { row.verdict = 'FAIL'; throw error; }
  };
  const adapter = {prepareTurn: async value => { armed = value; probes.push(value.configuration); },
    releaseTurn: async ({caseId}) => assert.equal(caseId, armed.caseId),
    oracle: async ({caseId}) => {
      assert.equal(caseId, armed.caseId);
      const {instructions, effort} = armed.configuration;
      const observedEffort = staleNativeEffort ? original.settings.reasoning_effort : settings.reasoning_effort;
      const observedPrompt = staleNativePrompt ? original.preferences.system_prompt : preferences.system_prompt;
      const matched = effort === observedEffort && (!instructions || observedPrompt.includes(instructions));
      return {case_id: caseId, request_count: 1, completed_effect_count: 1, active_streams: 0,
        configuration_matches: Number(matched)};
    }, titleEffects: async ({agentId, title}) => {
    assert.equal(agentId, record.agentId);
    assert.equal(title, preferences.title);
    return effect;
  }};
  return {api, adapter, record, runCase, recordFailure: async (stage, error) => failures.push({stage, message: error.message}),
    calls, rows, failures, original, probes, state: () => ({preferences, settings})};
}

test('configuration round trips change real values, preserve prompt, refuse stale writes and restore', async () => {
  const x = fixture(); await configurationCases(x);
  assert.equal(x.rows.length, 2); assert.ok(x.rows.every(row => row.verdict === 'PASS'));
  assert.equal(x.state().preferences.system_prompt, x.original.preferences.system_prompt);
  assert.deepEqual(x.state().settings, x.original.settings);
  const changed = x.calls.filter(row => row.method === 'PATCH');
  assert.equal(changed.filter(row => row.body.permission_mode === 'guarded').length, 1);
  assert.equal(changed.filter(row => row.body.permission_mode === 'autonomous').length, 1);
  assert.ok(changed.some(row => row.allowed?.includes(409)));
  assert.equal(new Set(changed.map(row => row.key)).size, changed.length);
  assert.match(x.probes[0].instructions, /^WS4_INSTRUCTIONS_[a-f0-9]{32}$/);
  assert.deepEqual(x.probes.map(probe => probe.effort), ['high', 'high', 'medium']);
});

test('durable API updates cannot pass when the native runtime keeps the previous configuration', async () => {
  for (const [flag, failedCase] of [['staleNativePrompt', 'user.system_prompt_roundtrip'],
    ['staleNativeEffort', 'user.session_settings_roundtrip']]) {
    const x = fixture({[flag]: true});
    await assert.rejects(configurationCases(x), /native_configuration_mismatch/);
    assert.deepEqual(x.rows.at(-1), {case: failedCase, verdict: 'FAIL'});
    assert.equal(x.state().preferences.system_prompt, x.original.preferences.system_prompt);
    assert.deepEqual(x.state().settings, x.original.settings);
  }
});

test('a successful status with the original prompt is not a passing mutation', async () => {
  const x = fixture({falseAck: true});
  await assert.rejects(configurationCases(x), /prompt_update_not_confirmed/);
  assert.equal(x.rows[0].verdict, 'FAIL');
  assert.deepEqual(x.failures, [{stage: 'primary', message: 'prompt_update_not_confirmed'}]);
});

test('lost prompt acknowledgement fails, restores via fresh revision and never repeats original intent', async () => {
  const x = fixture({lostAck: true});
  await assert.rejects(configurationCases(x), /lost_ack/);
  const writes = x.calls.filter(row => row.method === 'PATCH');
  assert.equal(writes.length, 2);
  assert.notEqual(writes[0].body.system_prompt, writes[1].body.system_prompt);
  assert.equal(writes[1].body.expected_revision, 2);
  assert.equal(x.state().preferences.system_prompt, x.original.preferences.system_prompt);
  assert.equal(x.rows[0].verdict, 'FAIL');
});

test('held-turn busy refusals preserve state while acknowledged changed rename survives owner move', async () => {
  const x = fixture({busy: true}); await heldConfigurationCases(x);
  assert.equal(x.rows.length, 2); assert.ok(x.rows.every(row => row.verdict === 'PASS'));
  assert.notEqual(x.record.writerTitle.title, x.original.preferences.title);
  assert.equal(x.record.writerTitle.revision, 2);
  await assertWriterTitle(x.api, x.adapter, x.record);
  const [first, replay, stale] = x.calls.filter(row => row.method === 'PATCH' && row.body.title);
  assert.equal(first.key, replay.key);
  assert.deepEqual(first.body, replay.body);
  assert.notEqual(stale.key, first.key);
  assert.equal(stale.prefer, 'cma-settings-reasons');
  assert.equal(x.state().preferences.revision, 2);
  const oldRead = x.api.request;
  x.api.request = async (...args) => {
    const response = await oldRead(...args); response.body.title = 'Lost acknowledged title'; return response;
  };
  await assert.rejects(assertWriterTitle(x.api, x.adapter, x.record), /acknowledged_title_lost_after_owner_move/);
});

test('fresh read and successful replay cannot hide duplicate durable rename effects', async () => {
  const effect = [3];
  const x = fixture({busy: true, effect});
  await heldConfigurationCases(x);
  effect.push(4);
  await assert.rejects(assertWriterTitle(x.api, x.adapter, x.record), /rename_effect_changed_or_repeated/);
});

test('busy refusal must be the typed business boundary and must not mutate state', async () => {
  for (const [options, error] of [
    [{badRefusal: true}, /busy_configuration_wrong_refusal/],
    [{mutateOnRefusal: true}, /busy_prompt_mutated/],
  ]) {
    const x = fixture({busy: true, ...options});
    await assert.rejects(heldConfigurationCases(x), error);
    assert.equal(x.calls.filter(row => row.method === 'PATCH' && row.body.title).length, 0);
  }
});

test('lifecycle replay keeps the acknowledged key and original body around changed-payload refusal', async () => {
  const record = {key: 'original-key', operationId: 'original-operation', createBody: {name: 'Original'}};
  const calls = [];
  const api = {request: async (am, method, path, options) => {
    calls.push({am, method, path, ...structuredClone(options)});
    return {body: options.allowed.includes(409) ? {code: 'agent_busy'} : {id: record.operationId}};
  }};
  await verifyLifecycleReplay(api, record);
  assert.equal(calls.length, 3); assert.ok(calls.every(call => call.key === record.key));
  assert.deepEqual(calls[0].body, record.createBody);
  assert.notDeepEqual(calls[1].body, record.createBody);
  assert.deepEqual(calls[2].body, record.createBody);
});

test('changed-payload refusal cannot poison the original admission identity', async () => {
  let count = 0;
  const record = {key: 'original-key', operationId: 'original-operation', createBody: {name: 'Original'}};
  const api = {request: async () => ({body: ++count === 1 ? {id: record.operationId}
    : count === 2 ? {code: 'agent_busy'} : {id: 'new-operation'}})};
  await assert.rejects(verifyLifecycleReplay(api, record), /altered_admission_replaced_original/);
  assert.equal(count, 3);
});
