import test from 'node:test';
import assert from 'node:assert/strict';
import {suggestionsCase} from './suggestions.mjs';

function fixture({duplicate = false, stalePublishes = false, lostTitleAck = false} = {}) {
  const agentId = 'fixture-agent', ownerId = 'fixture-owner', responseId = 'fixture-response';
  const scope = {turn_sequence: 3, answer_sequence: 5};
  const original = 'Original title';
  const statusReads = [], events = [], failures = [], mutations = [];
  let armed, input, answer, turnReleased = false, suggestionReleased = false;
  let preferences = {title: original, revision: 1};
  const observation = {failure: null, usage: {input_tokens: 1, output_tokens: 1}};
  const set = {id: 'fixture-suggestion', agentId, viewerId: ownerId, answerId: 'event:5',
    source: {kind: 'generated'}, scope,
    items: [{id: 'fixture-suggestion:0', rank: 0, text: 'Explain the fixture result in one sentence.'}]};
  const api = {
    config: {owner_id: ownerId},
    async send(am, target, text, key) {
      assert.equal(target, agentId);
      assert.equal(key, armed.caseId);
      assert.ok(text.includes(armed.marker) && turnReleased);
      input = text;
      answer = 'WS4 complete ' + armed.marker;
      return {responseId, abort: () => events.push('stream-aborted'), done: Promise.resolve()};
    },
    async rawRequest(am, method, path) {
      assert.equal(method, 'GET');
      if (path.endsWith('/preferences')) return {body: {...preferences}};
      if (path.endsWith(responseId)) {
        return {body: {id: responseId, conversation: {id: agentId}, status: 'completed', output: [answer]}};
      }
      if (path.endsWith('/transcript')) {
        return {body: {agent_id: agentId, turns: [{turn_sequence: 3, owner_message: input,
          outcome: {type: 'completed'}, messages: [{sequence: 5, role: 'assistant', content: answer}]}]}};
      }
      assert.equal(path, '/api/v1/agents/fixture-agent/suggestions?turn_sequence=3&answer_sequence=5');
      const stale = preferences.title !== original && !(stalePublishes && suggestionReleased);
      const status = stale ? 'expired' : suggestionReleased ? 'ready' : 'running';
      statusReads.push({am, status, suggestionReleased});
      return {body: {status, set: status === 'ready' ? set : null, observation: suggestionReleased ? observation : null}};
    },
    async request(am, method, path, {body, key}) {
      assert.equal(method, 'PATCH');
      assert.equal(path, '/api/v1/agents/fixture-agent/preferences');
      assert.equal(body.expected_revision, preferences.revision);
      assert.equal(mutations.some(previous => previous.key === key), false);
      mutations.push({body, key});
      preferences = {revision: preferences.revision + 1, title: body.title};
      if (lostTitleAck && mutations.length === 1) throw Error('lost_ack');
      return {body: {...preferences}};
    },
  };
  const adapter = {
    async prepareTurn(value) { armed = value; assert.equal(value.suggestions, true); },
    async releaseTurn({caseId, suggestion}) {
      assert.equal(caseId, armed.caseId);
      if (suggestion) suggestionReleased = true;
      else turnReleased = true;
    },
    async oracle({caseId}) {
      assert.equal(caseId, armed.caseId);
      return {case_id: caseId, request_count: 1, completed_effect_count: 1, active_streams: 0,
        suggestion_requests: suggestionReleased && duplicate ? 2 : 1,
        suggestion_responses: suggestionReleased ? 1 : 0, suggestion_released: suggestionReleased};
    },
    async titleEffects({agentId: target, title}) {
      assert.equal(target, agentId);
      assert.equal(title, preferences.title);
      return [7];
    },
  };
  return {api, adapter, record: {agentId}, statusReads, events, failures, mutations,
    state: () => ({suggestionReleased, preferences}),
    recordState: async (stage, state) => { events.push({stage, ...state}); },
    recordFailure: async (stage, error) => { failures.push([stage, error.message]); }};
}

test('the held answer publishes one set with the same scope on both Managers', async () => {
  const x = fixture();
  await suggestionsCase(x, false);
  assert.deepEqual(x.statusReads.filter(row => row.suggestionReleased).map(row => row.am), ['a', 'b', 'a']);
  assert.deepEqual(new Set(x.statusReads.map(row => row.status)), new Set(['running', 'ready']));
  assert.equal(x.mutations.length, 0);
  assert.equal(x.events[0].generationCount, 1);
});

test('a late suggestion is discarded after a real title change and the title is restored', async () => {
  const x = fixture();
  await suggestionsCase(x, true);
  assert.equal(x.events[0].status, 'expired');
  assert.deepEqual(x.mutations.map(row => row.body.expected_revision), [1, 2]);
  assert.deepEqual(x.state().preferences, {title: 'Original title', revision: 3});
  assert.ok(x.statusReads.some(row => row.status === 'expired' && !row.suggestionReleased));
  assert.ok(x.statusReads.filter(row => row.suggestionReleased).every(row => row.status === 'expired'));
});

test('stale or regenerated output fails and both failure paths release fixture custody', async () => {
  const stale = fixture({stalePublishes: true});
  await assert.rejects(suggestionsCase(stale, true), /stale_suggestion_published_after_title_change/);
  assert.equal(stale.state().preferences.title, 'Original title');
  assert.ok(stale.events.includes('stream-aborted'));
  const duplicate = fixture({duplicate: true});
  await assert.rejects(suggestionsCase(duplicate, false), /suggestion_generation_replayed/);
  assert.ok(duplicate.events.includes('stream-aborted'));
});

test('an uncertain title acknowledgement fails and restores using the observed revision', async () => {
  const x = fixture({lostTitleAck: true});
  await assert.rejects(suggestionsCase(x, true), /lost_ack/);
  assert.deepEqual(x.failures, [['primary', 'lost_ack']]);
  assert.equal(x.state().suggestionReleased, true);
  assert.deepEqual(x.state().preferences, {title: 'Original title', revision: 3});
});
