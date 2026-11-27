import assert from 'node:assert/strict';
import test from 'node:test';
import {generatedTitleCustody, titleCustodyAfterRestart} from './title_custody.mjs';

function fixture() {
  const record = {agentId: 'agent'}, custody = {digest: 'a'.repeat(64), requestCount: 1, effects: [6]};
  let armed, released = false, sent = false, scope;
  let preferences = {title: 'Original', revision: 1};
  const provenance = {agent_id: 'agent', creation_origin: 'manual', created_at: '2026-11-26T00:00:00Z',
    details: {trigger: null, channel_bridge: null}};
  const api = {
    async request(_am, method, path, options) {
      assert.equal(path, '/api/v1/agents/agent/preferences');
      if (method === 'PATCH') {
        assert(sent && !released);
        assert.equal(options.body.expected_revision, 1);
        preferences = {title: options.body.title, revision: 2};
      }
      return {body: preferences};
    },
    async send(_am, agentId, input, key) {
      assert.equal(agentId, record.agentId);
      assert.equal(key, armed.caseId);
      assert.ok(input.includes(armed.marker));
      sent = true;
      return {responseId: 'response', done: Promise.resolve(), abort() {}};
    },
    async rawRequest(_am, method, path) {
      assert.equal(method, 'GET');
      if (path.endsWith('/provenance')) return {body: {...provenance}};
      if (path.endsWith('/preferences')) return {body: preferences};
      if (path.endsWith('/transcript')) return {body: {agent_id: 'agent', turns: [{turn_sequence: 4,
        owner_message: 'Return ' + armed.marker}]}};
      assert.equal(path, '/api/v2/responses/response');
      return {body: {id: 'response', status: 'completed', conversation: {id: 'agent'}}};
    },
  };
  const adapter = {
    async prepareTurn(value) { assert.equal(value.title, true); armed = value; },
    async releaseTurn(value) { assert.equal(value.caseId, armed.caseId); if (value.title) released = true; },
    async oracle({caseId}) {
      return {case_id: caseId, request_count: 1, completed_effect_count: 1, active_streams: 0,
        title_requests: custody.requestCount, title_responses: Number(released), title_request_sha256: custody.digest};
    },
    async titleIntent(value) { scope = value;
      return {request_sha256: custody.digest, turn_sequence: 4}; },
    async titleEffects({agentId, title}) {
      assert.equal(agentId, scope.agentId);
      return title === preferences.title ? custody.effects : [];
    },
  };
  return {api, adapter, record, custody, provenance};
}

test('first title attempt remains the one retained provider effect across owner restart', async () => {
  const input = fixture();
  await generatedTitleCustody(input);
  assert.equal(input.record.generatedTitleProof.turnSequence, 4);
  await titleCustodyAfterRestart(input);
  input.custody.effects = [9];
  await assert.rejects(titleCustodyAfterRestart(input), /owner_title_effect_changed_after_restart/);
  input.custody.effects = [6];
  input.custody.requestCount += 1;
  await assert.rejects(titleCustodyAfterRestart(input), /title_model_effect_changed_or_repeated/);
});

test('a retained title result cannot hide changed immutable creation evidence', async () => {
  const input = fixture();
  await generatedTitleCustody(input);
  input.provenance.created_at = '2026-11-26T00:01:00Z';
  await assert.rejects(titleCustodyAfterRestart(input), /title_creation_provenance_changed/);
});
