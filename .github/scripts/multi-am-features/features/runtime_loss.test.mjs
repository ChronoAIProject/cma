import assert from 'node:assert/strict';
import test from 'node:test';
import {runtimeLoss} from './runtime_loss.mjs';

function fixture({changeHome = false, changeConversation = false, loseHistory = false, postEvents = 1} = {}) {
  const record = {agentId: 'agent', sandboxId: 'sandbox', operationId: 'create', runtime: {image: 'image'}};
  const prior = {status: 'active', boot_id: 'boot-a', children: [{status: 'ready', conversation_id: 'conversation',
    runtime_generation: 4, last_seq: 20}]};
  const after = {status: 'active', boot_id: 'boot-b', children: [{status: 'ready',
    conversation_id: changeConversation ? 'replacement' : 'conversation', runtime_generation: 5, last_seq: 35}]};
  const oldReply = {turn_sequence: 7, outcome: {type: 'completed'}, messages: [{content: 'retained'}]};
  const detail = {id: 'agent', status: 'ready', sandbox: {id: 'sandbox'},
    workspace: {id: 'workspace'}, worktree: {id: 'worktree'}};
  let removed = false, completed = false;
  const reads = [];
  const adapter = {
    ownerOf: async () => ({am: 'a', epoch: 1, agent_id: 'agent', pod_uid: 'owner'}),
    lifecycleSnapshot: async () => removed ? after : prior,
    runtimeFor: async () => ({ready: true, pod_uid: removed ? 'pod-b' : 'pod-a',
      sandbox_id: 'sandbox', image: 'image', home_identity: removed && changeHome ? 'other' : 'home', pvc_names: ['home']}),
    removeRuntimePod: async scope => {
      assert.deepEqual(scope, {agentId: 'agent', sandboxId: 'sandbox', operationId: 'create', podUid: 'pod-a'});
      assert(!removed);
      removed = true;
      return {accepted: true, pod_uid: 'pod-a'};
    },
  };
  const api = {rawRequest: async (am, method, path) => {
    assert.equal(method, 'GET');
    if (removed) reads.push([am, path]);
    if (path.endsWith('/transcript')) return {body: {agent_id: 'agent', turns: completed && loseHistory ? [] : [oldReply]}};
    if (path.endsWith('events?after=20')) return {body: {items: Array.from({length: postEvents}, () => ({kind: {
      type: 'runtime_generation_retired', previous_generation: 4, runtime_generation: 5,
    }}))}};
    assert.equal(path, '/api/v1/agents/agent');
    return {body: detail};
  }};
  return {adapter, api, record, reads,
    openBoth: async input => { assert.equal(input, record); return {both_am_details_and_transcripts: true}; },
    complete: async (input, am) => { assert.equal(input, record); assert.equal(am, 'b'); completed = true; },
    waitFor: async (read, accept) => { const value = await read(); assert(accept(value)); return value; }};
}

test('runtime loss proves replacement identity, retained history and a working peer Send', async () => {
  const context = fixture();
  const result = await runtimeLoss(context);
  assert.notEqual(result.previous_pod_sha256, result.replacement_pod_sha256);
  assert.equal(result.post_loss_reply, true);
  assert.equal(result.generation_retirements, 1);
  assert.deepEqual(context.reads.filter(([, path]) => path.includes('/events')).map(([am]) => am), ['a', 'b']);
});

test('a new Pod alone cannot pass while the home, conversation or event custody changes', async () => {
  for (const [input, error] of [[{changeHome: true}, /runtime_loss_home_replaced/],
    [{changeConversation: true}, /runtime_loss_conversation_replaced/],
    [{loseHistory: true}, /runtime_loss_history_changed/],
    [{postEvents: 2}, /runtime_loss_generation_retirement_unbound/]]) {
    await assert.rejects(runtimeLoss(fixture(input)), error);
  }
});
