import {once} from 'node:events';
import {createServer} from 'node:http';
import test from 'node:test';
import assert from 'node:assert/strict';
import {API} from '../cma/core.mjs';
import {requestPacer} from '../pacing.mjs';
import {observeHeldCommand, sharedIngressReceipt, verifyRecoveredReceipt} from './conversation_ingress.mjs';

function recovered({replayedCommand = 'fixture-command', epoch = 3, effects = 1, changedReceipt = false} = {}) {
  const agentId = 'fixture-agent', commandId = 'fixture-command', responseId = 'fixture-response';
  const owner = {am: 'b', epoch: 3, pod_uid: 'surviving-pod', agent_id: agentId};
  const receipt = {agent_id: agentId, command_id: commandId, response_id: responseId,
    admitted_at: '2026-11-26T00:00:00Z', agent_order: 5, event_cursor: 9};
  const turn = {caseId: 'fixture-case', input: 'synthetic message',
    controls: {response: {receipt}, target: {turn_sequence: 7}},
    stream: {commandId, responseId, timing: {request_id: 'original-request'}}};
  const completed = {id: responseId, conversation: {id: agentId}, status: 'completed'};
  const command = {status: 'completed', failure: null, turn_sequence: 7,
    receipt: changedReceipt ? {...receipt, event_cursor: 10} : receipt};
  const read = [], closed = [];
  const api = {
    rawRequest: async (am, method, route) => {
      assert.equal(method, 'GET');
      read.push(am);
      return {body: route.endsWith(commandId) ? command : completed};
    },
    send: async (am, agent, input, key) => {
      assert.deepEqual([am, agent, input, key], ['a', agentId, turn.input, turn.caseId]);
      return {ingress_am: am, commandId: replayedCommand, responseId, error: null,
        events: [{type: 'response.completed'}], timing: {request_id: 'replay-request'},
        done: Promise.resolve(), abort: () => closed.push(am)};
    },
    request: async (am, method, route, options) => {
      assert.deepEqual([am, method, route, options.key], ['a', 'POST', `/api/v2/agents/${agentId}/responses`, turn.caseId]);
      assert.deepEqual(options.allowed, [409]);
      assert.equal(options.direct, true);
      assert.equal(options.body.input, turn.input + ' Changed.');
      return {body: {code: 'idempotency_key_reused'}};
    },
  };
  const adapter = {ownerOf: async () => ({...owner, epoch}), oracle: async ({caseId}) => ({
    case_id: caseId, request_count: effects, completed_effect_count: effects, active_streams: 0})};
  return {api, adapter, agentId, owner, turn, completed, read, closed};
}

test('the restarted ingress replays the exact settled command without taking ownership', async () => {
  const context = recovered();
  const proof = await verifyRecoveredReceipt(context);
  assert.equal(proof.ingress, 'a');
  assert.match(proof.command_sha256, /^[a-f0-9]{64}$/);
  assert.deepEqual(context.read, ['b', 'a', 'a', 'b', 'b']);
  assert.deepEqual(context.closed, ['a']);
});

test('recovery replay fails on a second command, another owner epoch or a second native effect', async () => {
  for (const [options, error] of [[{replayedCommand: 'foreign'}, /recovered_admission_replay_changed/],
    [{epoch: 4}, /recovered_admission_owner_changed/], [{effects: 2}, /provider_request_count_mismatch/]]) {
    const context = recovered(options);
    await assert.rejects(verifyRecoveredReceipt(context), error);
    assert.deepEqual(context.closed, ['a']);
  }
});

test('the held command has the original receipt until the survivor can settle it', async () => {
  const {agentId, turn} = recovered();
  const held = {status: 'in_progress', failure: null, turn_sequence: 7, receipt: turn.controls.response.receipt};
  const api = {rawRequest: async (am, method, path) => {
    assert.deepEqual([am, method, path], ['b', 'GET', '/api/v2/commands/fixture-command']);
    return {body: held};
  }};
  await observeHeldCommand({api, agentId, ingress: 'b', turn});
  held.status = 'completed';
  await assert.rejects(observeHeldCommand({api, agentId, ingress: 'b', turn}), /held_command_settled_or_replaced/);
  await assert.rejects(verifyRecoveredReceipt(recovered({changedReceipt: true})), /recovered_command_receipt_unbound/);
});

async function fixture(t, {wrongRun = false, wrongReplay = false, recoveredRun = false} = {}) {
  t.mock.method(requestPacer, 'wait', async () => {});
  const agentId = 'fixture-agent', commandId = 'fixture-command', responseId = 'fixture-response';
  const calls = [], failures = [], outcomes = [];
  let armed, input, firstStream, released = false, answered = false;
  const command = {status: 'completed', failure: null, turn_sequence: 7,
    receipt: {agent_id: agentId, command_id: commandId, response_id: responseId}};
  const sendJson = (res, status, body) => res.writeHead(status, {'Content-Type': 'application/json'}).end(JSON.stringify(body));
  const frame = value => 'data: ' + JSON.stringify(value) + '\n\n';
  const readResponse = () => ({id: responseId, conversation: {id: agentId},
    status: 'completed', output: ['WS4 complete ' + armed.marker]});
  const agentResponses = `/api/v2/agents/${agentId}/responses`;
  if (recoveredRun) {
    armed = {caseId: 'settled-key', marker: 'settled-marker'};
    input = 'original input for ' + armed.marker;
    released = answered = true;
  }

  async function serve(am, req, res) {
    calls.push({am, route: req.url, method: req.method});
    if (req.method === 'POST') {
      let raw = '';
      for await (const chunk of req) raw += chunk;
      const body = JSON.parse(raw);
      assert.equal(req.headers['idempotency-key'], armed.caseId);
      if (req.url === '/api/v1/agui/run') {
        assert.equal(am, 'b');
        assert.equal(body.threadId, agentId);
        assert.equal(body.messages[0].role, 'user');
        input = body.messages[0].content;
        assert.ok(input.includes(armed.marker));
        firstStream = res;
        res.writeHead(200, {'Content-Type': 'text/event-stream', 'x-cma-command-id': commandId,
          'x-cma-response-id': responseId});
        res.write(frame({type: 'RUN_STARTED', threadId: wrongRun ? 'other-agent' : agentId, runId: '7'}));
        return;
      }
      assert.equal(req.url, recoveredRun ? agentResponses : '/api/v2/responses');
      if (!recoveredRun) assert.equal(body.conversation, agentId);
      assert.equal(body.stream, true);
      assert.ok(answered);
      if (body.input !== input) return sendJson(res, 409, {code: 'idempotency_key_reused'});
      assert.equal(am, 'a');
      res.writeHead(200, {'Content-Type': 'text/event-stream', 'x-cma-command-id': wrongReplay ? 'other-command' : commandId,
        'x-cma-response-id': responseId, 'x-request-id': 'replayed-request'});
      return res.end(frame({type: 'response.created', sequence_number: 0, response: readResponse()})
        + frame({type: 'response.completed', sequence_number: 1, response: readResponse()}));
    }
    if (req.url === '/api/v2/commands/' + commandId) return sendJson(res, 200, command);
    if (req.url === '/api/v2/responses/' + responseId) return sendJson(res, 200, readResponse());
    assert.equal(req.url, '/api/v1/agents/' + agentId + '/transcript');
    const markdown = `WS4 complete ${armed.marker}\n\n\`\`\`python\nfixture_marker = "${armed.marker}"\n\`\`\``;
    return sendJson(res, 200, {agent_id: agentId, turns: [{owner_message: input, turn_sequence: 7,
      agent_messages: [markdown]}]});
  }

  const ams = {};
  for (const am of ['a', 'b']) {
    const server = createServer((req, res) => {
      serve(am, req, res).catch(error => {
        failures.push(error);
        res.destroy();
      });
    });
    server.listen(0, '127.0.0.1');
    await once(server, 'listening');
    t.after(() => {server.closeAllConnections(); server.close();});
    ams[am] = 'http://127.0.0.1:' + server.address().port;
  }
  const api = new API({ams, bearer: 'synthetic-bearer', host: 'fixture.example', public_origin: 'https://fixture.example'});
  t.after(() => api.close());
  api.router = {dispatch: async (agent, run) => {assert.equal(agent, agentId); return run('b');}};
  const adapter = {
    async ownerOf() { return {am: 'b', epoch: 2, pod_uid: 'live-owner', agent_id: agentId}; },
    async prepareTurn(options) { armed = options; },
    async releaseTurn({caseId}) {
      assert.equal(caseId, armed.caseId);
      released = true;
      if (firstStream && !firstStream.writableEnded) {
        answered = true;
        firstStream.end(frame({type: 'RUN_FINISHED', threadId: agentId, runId: '7', outcome: {type: 'complete'}}));
      }
    },
    async oracle({caseId}) {
      assert.equal(caseId, armed.caseId);
      return {case_id: caseId, request_count: 1, completed_effect_count: answered ? 1 : 0,
        active_streams: answered ? 0 : 1, released};
    },
  };
  return {api, adapter, record: {agentId}, calls, failures, outcomes, released: () => released,
    priorTurn: recoveredRun ? {caseId: armed.caseId, input,
      controls: {response: {receipt: command.receipt}, target: {turn_sequence: command.turn_sequence}}, stream: {commandId, responseId,
      timing: {request_id: 'original-request'}}} : undefined,
    completed: recoveredRun ? readResponse() : undefined,
    recordState: async (stage, row) => { outcomes.push({stage, ...row}); },
    recordFailure: async (stage, error) => { failures.push([stage, error.message]); }};
}

test('the restarted ingress serves the frozen command over the real Responses transport', async t => {
  const context = await fixture(t, {recoveredRun: true});
  const {api, adapter, record: {agentId}} = context;
  api.routingEnabled = false;
  const owner = await adapter.ownerOf();
  const proof = await verifyRecoveredReceipt({api, adapter, agentId, owner, turn: context.priorTurn,
    completed: context.completed});
  assert.deepEqual(context.failures, []);
  assert.equal(proof.ingress, 'a');
  assert.deepEqual(context.calls.filter(row => row.method === 'POST').map(row => [row.am, row.route]),
    Array.from({length: 2}, () => ['a', `/api/v2/agents/${agentId}/responses`]));
});

test('AG-UI admission and root Responses replay share one command on distinct ingresses', async t => {
  const fixtureState = await fixture(t);
  await sharedIngressReceipt(fixtureState);
  assert.deepEqual(fixtureState.failures, []);
  assert.deepEqual(fixtureState.calls.filter(row => row.method === 'POST').map(row => [row.am, row.route]),
    [['b', '/api/v1/agui/run'], ['a', '/api/v2/responses'], ['b', '/api/v2/responses']]);
  assert.equal(fixtureState.outcomes[0].effects, 1);
  assert.equal(fixtureState.outcomes[0].command_sha256.length, 64);
});

test('a mismatched AG-UI thread or root Responses receipt fails and releases the provider', async t => {
  for (const [options, error] of [
    [{wrongRun: true}, /agui_run_receipt_invalid/],
    [{wrongReplay: true}, /root_responses_replay_changed_receipt/],
  ]) {
    const fixtureState = await fixture(t, options);
    await assert.rejects(sharedIngressReceipt(fixtureState), error);
    assert.equal(fixtureState.released(), true);
    assert.equal(fixtureState.outcomes.length, 0);
  }
});
