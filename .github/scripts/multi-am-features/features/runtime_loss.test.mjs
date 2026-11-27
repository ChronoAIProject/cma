import assert from 'node:assert/strict';
import test from 'node:test';
import {runtimeLoss} from './runtime_loss.mjs';

function fixture({changeHome = false, changeConversation = false, loseHistory = false,
  postEvents = 1, mixedGenerations = false} = {}) {
  const record = {agentId: 'agent', sandboxId: 'sandbox', operationId: 'create', runtime: {image: 'image'}};
  const sibling = {agentId: 'sibling', sandboxId: 'sandbox'};
  const prior = {status: 'active', boot_id: 'boot-a', children: [record, sibling].map(agent => ({status: 'ready',
    conversation_id: 'conversation-' + agent.agentId, runtime_generation: 4, last_seq: 20}))};
  const after = {status: 'active', boot_id: 'boot-b', children: prior.children.map((agent, index) => ({...agent,
    conversation_id: index === 1 && changeConversation ? 'replacement' : agent.conversation_id,
    runtime_generation: index === 1 && mixedGenerations ? 4 : 5, last_seq: 35}))};
  const oldReply = {turn_sequence: 7, outcome: {type: 'completed'}, messages: [{content: 'retained'}]};
  let removed = false;
  const reads = [], completions = [];
  const adapter = {
    ownerOf: async ({agentId}) => ({am: agentId === 'agent' ? 'a' : 'b', epoch: 1,
      agent_id: agentId, pod_uid: 'owner-' + agentId}),
    lifecycleSnapshot: async scope => {
      assert.deepEqual(scope, {sandboxId: 'sandbox', agentIds: ['agent', 'sibling']});
      return removed ? after : prior;
    },
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
    const agentId = path.split('/')[4];
    assert(['agent', 'sibling'].includes(agentId));
    if (path.endsWith('/transcript')) return {body: {agent_id: agentId,
      turns: removed && agentId === 'sibling' && loseHistory ? [] : [oldReply]}};
    if (path.endsWith('events?after=20')) return {body: {items: Array.from({length: postEvents}, () => ({kind: {
      type: 'runtime_generation_retired', previous_generation: 4, runtime_generation: 5,
    }}))}};
    assert.equal(path, '/api/v1/agents/' + agentId);
    return {body: {id: agentId, status: 'ready', sandbox: {id: 'sandbox'}, workspace: {id: 'workspace'},
      worktree: {id: 'worktree-' + agentId}}};
  }};
  return {adapter, api, record, sibling, reads, completions,
    openBoth: async input => { assert([record, sibling].includes(input)); return {both_am_details_and_transcripts: true}; },
    complete: async (input, am) => completions.push([input.agentId, am, removed]),
    waitFor: async (read, accept) => { const value = await read(); assert(accept(value)); return value; }};
}

test('runtime loss retains both Agents with one replacement and a working peer Send per Agent', async () => {
  const context = fixture();
  const result = await runtimeLoss(context);
  assert.notEqual(result.previous_pod_sha256, result.replacement_pod_sha256);
  assert.equal(result.recovered_agents, 2);
  assert.equal(result.post_loss_replies, 2);
  assert.equal(result.generation_retirements, 2);
  assert.deepEqual(context.reads.filter(([, path]) => path.includes('/events')), [
    ['a', '/api/v1/agents/agent/events?after=20'], ['b', '/api/v1/agents/agent/events?after=20'],
    ['a', '/api/v1/agents/sibling/events?after=20'], ['b', '/api/v1/agents/sibling/events?after=20'],
  ]);
  assert.deepEqual(context.completions, [['sibling', 'a', false], ['agent', 'b', true], ['sibling', 'a', true]]);
});

test('a new Pod alone cannot pass while the home, conversation or event custody changes', async () => {
  for (const [input, error] of [[{changeHome: true}, /runtime_loss_home_replaced/],
    [{changeConversation: true}, /runtime_loss_conversation_replaced/],
    [{loseHistory: true}, /runtime_loss_history_changed/],
    [{postEvents: 2}, /runtime_loss_generation_retirement_unbound/],
    [{mixedGenerations: true}, /runtime_loss_partial_generation_retirement/]]) {
    await assert.rejects(runtimeLoss(fixture(input)), error);
  }
});
