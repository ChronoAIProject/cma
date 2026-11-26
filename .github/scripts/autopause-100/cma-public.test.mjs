import test from 'node:test';
import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {hash} from '../scale-tests/lib/checks.mjs';
import {fixture as nativeFixture, server} from './fixtures/native.mjs';
import {runCmaCampaign} from './cma-public.mjs';

async function campaignFixture(t, fault) {
  const data = nativeFixture('Running');
  const agentId = 'agt_campaign';
  const sandboxId = data.binding.sandbox_id;
  const ownerBearer = randomUUID();
  const adminBearer = randomUUID();
  const file = Buffer.from('original home marker');
  const completedTurn = (sequence, input, reply, replySequence = sequence + 2) => ({
    turn_sequence: sequence, owner_message: input, agent_messages: [reply],
    messages: [{sequence: sequence + 1, role: 'user', content: input},
      {sequence: replySequence, role: 'assistant', content: reply}],
    outcome: {type: 'completed', at: '2026-11-24T00:00:00Z'},
  });
  const turns = [completedTurn(2, 'Set up the original home marker.', 'Ready', 34)];
  const responses = new Map();
  let alwaysOn = false;
  let revision = 1;
  let cleanup = 0;
  let validated = false;
  let active;
  let elapsed = 0;
  let sweeps = 0;
  let pendingSamples = 0;
  let observedRunningAfterPending = 0;
  function setMode(mode) {
    data.sandbox.metadata.generation += 1;
    data.sandbox.metadata.resourceVersion = String(data.sandbox.metadata.generation);
    data.sandbox.spec.operatingMode = mode;
    data.pod.status.phase = 'Running';
    delete data.pod.metadata.deletionTimestamp;
    data.sandbox.status.conditions = [{type: mode === 'Running' ? 'Ready' : 'Suspended',
      status: 'True', observedGeneration: data.sandbox.metadata.generation}];
    data.pods.items = mode === 'Running' ? [data.pod] : [];
  }
  function finish(record) {
    record.response.status = 'completed';
    record.command.status = 'completed';
    const turn = completedTurn(record.command.turn_sequence, record.input, record.expected);
    if (fault === 'missing_user_message') {
      turn.messages = turn.messages.filter(message => message.role !== 'user');
    }
    turns.push(turn);
    active = undefined;
  }
  const publicSandbox = () => ({id: sandboxId, revision, always_on: alwaysOn,
    status: data.sandbox.spec.operatingMode === 'Running' ? 'active' : 'paused'});
  function json(res, value, status = 200) {
    res.writeHead(status, {'content-type': 'application/json'});
    res.end(JSON.stringify(value));
  }
  async function body(req) {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    return JSON.parse(Buffer.concat(chunks));
  }
  function transcriptPage(url, res) {
    const query = url.searchParams;
    if (query.get('limit') !== '32') {
      return json(res, {code: 'transcript_pagination_required'}, 400);
    }
    const through = Math.max(35, turns.at(-1).turn_sequence + 3);
    if (query.has('through')) assert.equal(Number(query.get('through')), through);
    const index = query.has('turn') ? turns.findIndex(turn => turn.turn_sequence === Number(query.get('turn'))) : 0;
    assert.ok(index >= 0);
    const turn = turns[index];
    const after = Number(query.get('after') ?? turn.turn_sequence - 1);
    const splitFirst = index === 0 && after < 33;
    const messages = turn.messages.filter(message => message.sequence > after && (!splitFirst || message.sequence <= 33));
    const fragment = {...turn, messages,
      agent_messages: messages.filter(message => message.role === 'assistant').map(message => message.content)};
    const next = splitFirst ? {turn: turn.turn_sequence, after: 33}
      : (turns[index + 1] && {turn: turns[index + 1].turn_sequence, after: turns[index + 1].turn_sequence - 1});
    const continuationFault = responses.size > 0 && query.has('through');
    return json(res, {agent_id: agentId,
      through_seq: through + Number(continuationFault && fault === 'high_water'),
      turns: continuationFault && fault === 'incomplete' ? [] : [fragment],
      next: next && {...next, through, limit: 32}});
  }
  async function publicRoute(req, res) {
    const url = new URL(req.url, 'http://127.0.0.1');
    const {pathname: path} = url;
    if (req.method === 'GET' && path === '/api/v1/admin/settings') {
      assert.equal(req.headers.authorization, `Bearer ${adminBearer}`);
      return json(res, {settings: {inactivity: {auto_pause_enabled: fault !== 'policy', pause_after_seconds: 900}}});
    }
    if (req.method === 'GET' && path === `/api/v1/agents/${agentId}`) {
      return json(res, {id: agentId, status: 'ready', sandbox: publicSandbox()});
    }
    if (req.method === 'GET' && path === `/api/v1/sandboxes/${sandboxId}`) {
      return json(res, publicSandbox());
    }
    if (req.method === 'GET' && path === `/api/v2/conversations/${agentId}`) {
      return json(res, {object: 'conversation', id: agentId, created_at: 1, metadata: {}});
    }
    if (req.method === 'GET' && path === `/api/v1/agents/${agentId}/transcript`) {
      return transcriptPage(url, res);
    }
    if (req.method === 'GET' && path === `/api/v1/agents/${agentId}/files/content`) {
      const bytes = fault === 'file' && responses.size ? Buffer.from('replaced marker') : file;
      return json(res, {size_bytes: bytes.length, content_base64: bytes.toString('base64'), sha256: hash(bytes)});
    }
    if (req.method === 'POST' && path === `/api/v1/sandboxes/${sandboxId}/always-on`) {
      return json(res, {code: 'always_on_approval_required'}, 403);
    }
    if (req.method === 'POST' && path === `/api/v1/admin/sandboxes/${sandboxId}/always-on`) {
      assert.equal(req.headers.authorization, `Bearer ${adminBearer}`);
      const change = await body(req);
      assert.equal(change.expected_revision, revision);
      revision += 1;
      alwaysOn = change.always_on;
      return json(res, publicSandbox());
    }
    if (req.method === 'POST' && path === `/api/v2/agents/${agentId}/responses`) {
      if (fault === 'redirect') {
        res.writeHead(302, {location: '/unexpected'});
        return res.end();
      }
      assert.equal(req.headers.authorization, `Bearer ${ownerBearer}`);
      assert.equal(data.sandbox.spec.operatingMode, 'Suspended');
      const input = await body(req);
      assert.equal(input.stream, true);
      const receiptKey = req.headers['idempotency-key'];
      assert.equal(responses.has(receiptKey), false);
      const index = responses.size + 1;
      const expected = `CMA-RETAINED-CYCLE-${index}`;
      assert.equal(input.input, `Reply with exactly ${expected}.`);
      const response = {object: 'response', id: `resp_${index}`, conversation: {id: agentId},
        status: 'in_progress', output: fault === 'empty_output' ? [] : [{
          type: 'message', role: 'assistant', status: 'completed',
          content: [{type: 'output_text', text: fault === 'wrong_output' ? 'unrelated reply' : expected}],
        }]};
      const command = {object: 'cma.agent.command', status: 'in_progress', turn_sequence: 36 + (index - 1) * 4,
        receipt: {agent_id: agentId, command_id: `cmd_${index}`, response_id: response.id, agent_order: index}};
      const record = {response, command, input: input.input, expected};
      responses.set(receiptKey, record);
      active = record;
      setMode('Running');
      if (fault === 'pending') data.pod.status.phase = 'Pending';
      if (fault === 'dead_runtime') {
        data.pod.status.phase = 'Failed';
        data.pod.metadata.deletionTimestamp = '2026-11-24T00:00:00Z';
      }
      if (index > 1) finish(record);
      res.writeHead(200, {'content-type': 'text/event-stream',
        'x-cma-response-id': response.id, 'x-cma-command-id': command.receipt.command_id});
      return res.end('event: response.created\n\n');
    }
    if (req.method === 'POST' && path === `/api/v2/agents/${agentId}/responses/receipt`) {
      const record = responses.get(req.headers['idempotency-key']);
      assert.equal((await body(req)).input, record.input);
      return json(res, fault === 'receipt' ? null : record.command);
    }
    for (const {response, command} of responses.values()) {
      if (path === `/api/v2/responses/${response.id}`) return json(res, response);
      if (path === `/api/v2/commands/${command.receipt.command_id}`) return json(res, command);
    }
    return json(res, null, 404);
  }
  const http = await server(t, data, (req, res) => {
    if (req.url.endsWith('/pods?limit=128') && fault === 'pending' && responses.size > 0) {
      if (data.pod.status.phase === 'Pending') {
        pendingSamples += 1;
        if (pendingSamples === 2) data.pod.status.phase = 'Running';
      } else if (data.pods.items.length) {
        observedRunningAfterPending += 1;
      }
    }
    if (Object.hasOwn(data.routes, req.url)) return false;
    publicRoute(req, res).catch(error => res.destroy(error));
    return true;
  });
  const options = {origin: http.origin, nativeOrigin: http.origin,
    ownerBearer, adminBearer, nativeBearer: randomUUID(),
    intervalMs: 1, requestMs: 3000, idleMs: 3000, turnMs: 3000,
    resumeMs: fault === 'dead_runtime' ? 50 : 3000,
    now: () => elapsed,
    fixture: {
      async validate() { validated = true; },
      async prepare() {
        assert.equal(validated, true);
        return {agentId, sandboxId, filePath: 'campaign-marker.txt', binding: data.binding};
      },
      async idleOpportunity() {
        if (fault !== 'no_reconcile') sweeps += 1;
        if (!alwaysOn && !active) setMode('Suspended');
      },
      async waitForInactivity(ms) {
        if (fault !== 'short_exception' && (fault !== 'short_turn' || !active)) elapsed += ms;
      },
      async reconciliationCount() { return sweeps; },
      async holdFirstTurn(sample) {
        await sample();
        finish(active);
      },
      async cleanup() {
        cleanup += 1;
        if (fault === 'cleanup') throw new Error('fixture cleanup failed');
      },
    }};
  return {options, http, responses, cleanupCount: () => cleanup,
    runtimeSamples: () => ({pendingSamples, observedRunningAfterPending})};
}

test('100 HTTP fixture cycles preserve the original home, transcript and exact receipts', {timeout: 90000}, async t => {
  const f = await campaignFixture(t);
  const packets = [];
  const result = await runCmaCampaign(f.options, {record: packet => packets.push(packet)});
  assert.equal(result.summary.passed, 100);
  assert.equal(result.summary.failed, 0);
  assert.equal(result.summary.requested, 100);
  assert.equal(result.summary.qualification, 'NOT_ASSESSED');
  assert.equal(result.final_verification, 'PASS');
  assert.equal(result.cycles[0].active_turn.hold_ms, 900000);
  assert.equal(packets.length, 101);
  assert.equal(f.responses.size, 100);
  assert.equal(f.cleanupCount(), 1);
  assert.equal(f.http.requests.filter(row => row.method === 'POST'
    && row.url.endsWith('/responses')).length, 100);
  assert.equal(f.http.requests.some(row => /pause-idle-now|\/pause|\/revive/.test(row.url)), false);
  assert.ok(f.http.requests.some(row => row.url.includes('turn=2&after=33')));
  assert.ok(f.http.requests.some(row => row.url.includes('through=435')));
});

for (const [fault, stage] of [['file', 'retention_failed'], ['receipt', 'complete_turn_failed'],
  ['redirect', 'message_wake_failed'], ['dead_runtime', 'message_wake_failed'],
  ['empty_output', 'complete_turn_failed'], ['wrong_output', 'complete_turn_failed'],
  ['high_water', 'complete_turn_failed'], ['incomplete', 'complete_turn_failed'],
  ['missing_user_message', 'complete_turn_failed'],
  ['short_turn', 'active_turn_failed']]) {
  test(`the campaign records ${fault} failure without a replacement cycle`, async t => {
    const f = await campaignFixture(t, fault);
    const packets = [];
    await assert.rejects(runCmaCampaign(f.options, {count: 2, record: packet => packets.push(packet)}),
      new RegExp(stage));
    assert.equal(f.cleanupCount(), 1);
    assert.equal(packets.at(-1).summary.attempted, 1);
    assert.equal(packets.at(-1).summary.failed, 1);
    assert.equal(f.http.requests.some(row => row.url === '/unexpected'), false);
  });
}

test('wake waits for a Pending Pod to become Running', async t => {
  const f = await campaignFixture(t, 'pending');
  const result = await runCmaCampaign(f.options, {count: 1, record() {}});
  assert.equal(result.summary.passed, 1);
  assert.equal(f.runtimeSamples().pendingSamples, 2);
  assert.ok(f.runtimeSamples().observedRunningAfterPending > 0);
});

for (const fault of ['short_exception', 'no_reconcile']) {
  test(`the Always-on observation refuses ${fault}`, async t => {
    const f = await campaignFixture(t, fault);
    const packets = [];
    await assert.rejects(runCmaCampaign(f.options, {count: 1, record: packet => packets.push(packet)}),
      /preparation_failed/);
    assert.equal(packets[0].summary.attempted, 0);
    assert.equal(f.responses.size, 0);
  });
}

test('the adapter requires enabled inactivity before provisioning', async t => {
  const f = await campaignFixture(t, 'policy');
  const packets = [];
  await assert.rejects(runCmaCampaign(f.options, {count: 1, record: packet => packets.push(packet)}),
    /preparation_failed/);
  assert.equal(packets[0].summary.attempted, 0);
  assert.equal(f.cleanupCount(), 1);
});

test('cleanup failure suppresses a completed campaign PASS', async t => {
  const f = await campaignFixture(t, 'cleanup');
  const packets = [];
  await assert.rejects(runCmaCampaign(f.options, {count: 1, record: packet => packets.push(packet)}),
    /cleanup_failed/);
  assert.equal(packets.some(packet => packet.verdict === 'PASS'), false);
  assert.equal(packets.at(-1).summary.passed, 1);
  assert.equal(packets.at(-1).summary.verdict, 'FAIL');
  assert.equal(packets.at(-1).error_code, 'cleanup_failed');
});
