import {once} from 'node:events';
import {createServer} from 'node:http';
import test from 'node:test';
import assert from 'node:assert/strict';
import {API} from '../cma/core.mjs';
import {requestPacer} from '../pacing.mjs';
import {sharedIngressReceipt} from './conversation_ingress.mjs';

async function fixture(t, {wrongRun = false, wrongReplay = false} = {}) {
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
      assert.equal(req.url, '/api/v2/responses');
      assert.equal(body.conversation, agentId);
      assert.equal(body.stream, true);
      assert.ok(answered);
      if (body.input !== input) return sendJson(res, 409, {code: 'idempotency_key_reused'});
      assert.equal(am, 'a');
      res.writeHead(200, {'Content-Type': 'text/event-stream', 'x-cma-command-id': wrongReplay ? 'other-command' : commandId,
        'x-cma-response-id': responseId});
      return res.end(frame({type: 'response.created', sequence_number: 0, response: readResponse()})
        + frame({type: 'response.completed', sequence_number: 1, response: readResponse()}));
    }
    if (req.url === '/api/v2/commands/' + commandId) return sendJson(res, 200, command);
    if (req.url === '/api/v2/responses/' + responseId) return sendJson(res, 200, readResponse());
    assert.equal(req.url, '/api/v1/agents/' + agentId + '/transcript');
    return sendJson(res, 200, {agent_id: agentId, turns: [{owner_message: input, turn_sequence: 7}]});
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
    recordState: async (stage, row) => { outcomes.push({stage, ...row}); },
    recordFailure: async (stage, error) => { failures.push([stage, error.message]); }};
}

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
