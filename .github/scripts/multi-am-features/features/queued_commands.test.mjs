import test from 'node:test';
import assert from 'node:assert/strict';
import {queuedCommandCase} from './queued_commands.mjs';

function fixture({duplicateEffect = false, falseEdit = false, unknownEdit = false, forgedReplay = false} = {}) {
  const receipt = {agent_id: 'agent', response_id: 'response', command_id: 'command', agent_order: 2};
  let revision = 1, text = 'original', edit, cancelled, aborted = false;
  const calls = [], states = [];
  const response = () => ({receipt, status: cancelled ? 'cancelled' : 'queued',
    turn_sequence: null, delivery: null, observed_cursor: null, failure: null});
  const api = {
    send: async (...args) => { calls.push({kind: 'send', args}); return {
      responseId: 'response', abort: () => { aborted = true; }, done: Promise.resolve()}; },
    request: async (_am, method, path, options) => {
      calls.push({method, path, options: structuredClone(options)});
      if (path.endsWith('/queued-inputs')) return {body: {data: [{response: response(), editable: true,
        command_revision: revision, text}]}};
      if (path.endsWith('/queued-input')) {
        if (unknownEdit) throw Object.assign(Error('unknown_commit'), {response: {status: 503}});
        if (options.allowed?.includes(409)) return {body: {error: {code: 'queued_input_not_admitted'}}};
        if (!edit) {
          if (!falseEdit) { text = options.body.text; revision++; }
          edit = {response: response(), edit: {status: 'completed'}, text, command_revision: revision};
        } else if (forgedReplay) return {body: {...edit, response: {...response(), receipt: {...receipt, command_id: 'different'}}}};
        return {body: structuredClone(edit)};
      }
      if (path.endsWith('/queued-cancellation')) {
        if (method === 'GET') return {body: {response: response(), cancellable: true, command_revision: revision}};
        cancelled ??= {cancellation: {...response(), status: 'completed'}, response: {...response(), status: 'cancelled'}};
        return {body: structuredClone(cancelled)};
      }
      assert.equal(path, '/api/v2/responses/response');
      return {body: {id: 'response', conversation: {id: 'agent'}, status: 'incomplete',
        incomplete_details: {reason: 'interrupted'}, output: []}};
    }};
  return {api, record: {agentId: 'agent'}, turn: {caseId: 'held'},
    adapter: {oracle: async () => ({case_id: 'held', request_count: duplicateEffect ? 2 : 1,
      active_streams: 1, completed_effect_count: 0})},
    recordState: async (stage, value) => states.push({stage, value: structuredClone(value)}),
    calls, states, aborted: () => aborted};
}

test('queued edits and cancellation preserve receipts, exact replay and active-effect custody', async () => {
  const x = fixture(); await queuedCommandCase(x);
  assert.equal(x.calls.filter(row => row.kind === 'send').length, 1);
  assert.equal(x.states.length, 3); assert.equal(x.aborted(), true);
  const edits = x.calls.filter(row => row.path?.endsWith('/queued-input'));
  assert.deepEqual(edits[0].options, edits[1].options);
  assert.notEqual(edits[2].options.key, edits[0].options.key);
  const cancels = x.calls.filter(row => row.path?.endsWith('/queued-cancellation') && row.method === 'POST');
  assert.deepEqual(cancels[0].options, cancels[1].options);
  assert.equal(cancels[0].options.body.expected_command_revision, 2);
});

test('false acknowledgement, changed replay and duplicate native effect fail the case', async () => {
  for (const [options, expected] of [[{falseEdit: true}, /queued_edit_not_durable/],
    [{forgedReplay: true}, /queued_edit_replay_changed_receipt/],
    [{duplicateEffect: true}, /queued_mutation_changed_active_provider_effect/]]) {
    const x = fixture(options);
    await assert.rejects(queuedCommandCase(x), expected);
    assert.equal(x.aborted(), true);
  }
});

test('unknown edit outcome retains error and never issues replay, rekey or cancellation', async () => {
  const x = fixture({unknownEdit: true});
  await assert.rejects(queuedCommandCase(x), error => error.message === 'unknown_commit' && error.response.status === 503);
  assert.equal(x.calls.filter(row => row.method === 'POST').length, 1);
  assert.equal(x.aborted(), true);
});
