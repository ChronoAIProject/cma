import assert from 'node:assert/strict';
import test from 'node:test';
import {hash} from '../cma/core.mjs';
import {runtimeLoss} from './runtime_loss.mjs';

function fixture({changeHome = false, changeConversation = false, loseHistory = false,
  postEvents = 1, mixedGenerations = false, aborts = {a: 1, b: 0}, consumedAborts = 1,
  scheduled = {a: 0, b: 1},
  missWindow = false, loseTitle = false, titleSeq = 23, transactionEntered = true,
  writerOnIngress = false, writerNotLogged = false, replaceManager = false, repeatAbort = false} = {}) {
  const record = {agentId: 'agent', sandboxId: 'sandbox', operationId: 'create', runtime: {image: 'image'}};
  const sibling = {agentId: 'sibling', sandboxId: 'sandbox'};
  const prior = {status: 'active', boot_id: 'boot-a', children: [record, sibling].map(agent => ({status: 'ready',
    conversation_id: 'conversation-' + agent.agentId, runtime_generation: 4, last_seq: 20}))};
  const after = {status: 'active', boot_id: 'boot-b', children: prior.children.map((agent, index) => ({...agent,
    conversation_id: index === 1 && changeConversation ? 'replacement' : agent.conversation_id,
    runtime_generation: index === 1 && mixedGenerations ? 4 : 5, last_seq: 35}))};
  const oldReply = {turn_sequence: 7, outcome: {type: 'completed'}, messages: [{content: 'retained'}]};
  let removed = false;
  let observationsWithWriter = 0;
  let title;
  let topologyReads = 0;
  const members = Object.fromEntries([['a', 'agent'], ['b', 'sibling']].map(([am, agent]) => [am, {
    ready: true, instance_id: 'manager-' + am, pod_uid: 'owner-' + agent,
  }]));
  const reads = [], completions = [], faultActions = [];
  const adapter = {
    ownerOf: async ({agentId}) => {
      const am = agentId === 'agent' ? 'a' : 'b';
      return {...members[am], am, epoch: 1, agent_id: agentId};
    },
    topology: async () => ({coordination_enabled: true, shared_database: true, ams: {
      ...members, a: {...members.a, ...(++topologyReads === 3 && replaceManager ? {pod_uid: 'replaced'} : {})},
    }}),
    peerWriteEvidence: async ({am, requestHash, epoch, operation}) => {
      assert.equal(requestHash, hash('preferences-request'));
      assert.equal(epoch, 1);
      assert.equal(operation, 'preferences');
      return {matches: am === 'a' ? Number(writerOnIngress) : Number(!writerNotLogged)};
    },
    lifecycleSnapshot: async scope => {
      assert.deepEqual(scope, {sandboxId: 'sandbox', agentIds: ['agent', 'sibling']});
      return removed ? after : prior;
    },
    runtimeFor: async () => ({ready: true, pod_uid: removed ? 'pod-b' : 'pod-a',
      sandbox_id: 'sandbox', image: 'image', home_identity: removed && changeHome ? 'other' : 'home', pvc_names: ['home']}),
    removeRuntimePod: async scope => {
      assert.deepEqual(scope, {agentId: 'agent', sandboxId: 'sandbox', operationId: 'create', podUid: 'pod-a'});
      assert(!removed);
      assert.deepEqual(faultActions, ['arm']);
      removed = true;
      return {accepted: true, pod_uid: 'pod-a'};
    },
    runtimeLossEvidence: async scope => {
      assert.deepEqual(scope, {sandboxId: 'sandbox'});
      if (title) observationsWithWriter += 1;
      const visible = title && (missWindow || observationsWithWriter > 1);
      const repeated = repeatAbort && observationsWithWriter > 2;
      return Object.fromEntries(['a', 'b'].map(am => [am, {
        known_aborts: (visible ? aborts[am] : 0) + Number(am === 'a' && repeated),
        recovery_scheduled: observationsWithWriter > 2 ? scheduled[am] : 0,
      }]));
    },
    runtimeLossPublicationAbort: async ({action, count}) => {
      faultActions.push(action);
      if (action === 'wait') {
        assert.equal(count, 3);
        assert(removed);
        return {entered: transactionEntered};
      }
      return {count: action === 'arm' ? 3 : 3 + consumedAborts};
    },
    titleEffects: async ({agentId, title: expected}) => {
      assert.equal(agentId, 'sibling');
      assert.equal(expected, title);
      return [titleSeq];
    },
  };
  const api = {rawRequest: async (am, method, path, options) => {
    if (method === 'PATCH') {
      assert.deepEqual(faultActions, ['arm', 'wait']);
      assert.equal(am, 'a');
      assert.equal(path, '/api/v1/agents/sibling/preferences');
      assert.equal(options.body.expected_revision, 5);
      title = options.body.title;
      return {status: 200, body: {title, revision: 6}, timing: {am, method,
        route: '/api/v1/agents/{id}/preferences', request_id: 'preferences-request'}};
    }
    assert.equal(method, 'GET');
    if (removed) reads.push([am, path]);
    const agentId = path.split('/')[4];
    assert(['agent', 'sibling'].includes(agentId));
    if (path.endsWith('/preferences')) return {body: title && !loseTitle ? {title, revision: 6} : {revision: 5}};
    if (path.endsWith('/transcript')) return {body: {agent_id: agentId,
      turns: removed && agentId === 'sibling' && loseHistory ? [] : [oldReply]}};
    if (path.endsWith('events?after=20')) return {body: {items: Array.from({length: postEvents}, () => ({seq: 28, kind: {
      type: 'runtime_generation_retired', previous_generation: 4, runtime_generation: 5,
    }}))}};
    assert.equal(path, '/api/v1/agents/' + agentId);
    return {body: {id: agentId, status: 'ready', sandbox: {id: 'sandbox'}, workspace: {id: 'workspace'},
      worktree: {id: 'worktree-' + agentId}}};
  }};
  return {adapter, api, record, sibling, reads, completions, faultActions,
    openBoth: async input => { assert([record, sibling].includes(input)); return {both_am_details_and_transcripts: true}; },
    complete: async (input, am) => completions.push([input.agentId, am, removed]),
    waitFor: async (read, accept, {code}) => {
      const value = await read();
      if (!accept(value)) throw new Error(code);
      return value;
    }};
}

test('runtime loss retains both Agents with one replacement and a working peer Send per Agent', async () => {
  const context = fixture();
  const result = await runtimeLoss(context);
  assert.notEqual(result.previous_pod_sha256, result.replacement_pod_sha256);
  assert.equal(result.recovered_agents, 2);
  assert.equal(result.post_loss_replies, 2);
  assert.equal(result.generation_retirements, 2);
  assert.equal(result.known_recovery_aborts, 1);
  assert.equal(result.competing_metadata_publications, 1);
  assert.equal(result.recovery_abort_manager, 'a');
  assert.equal(result.recovery_abort_instance_sha256, hash('manager-a'));
  assert.equal(result.recovery_commit_manager, 'b');
  assert.equal(result.recovery_commit_instance_sha256, hash('manager-b'));
  assert.equal(result.metadata_ingress_instance_sha256, hash('manager-a'));
  assert.equal(result.metadata_writer_instance_sha256, hash('manager-b'));
  assert.equal(result.metadata_request_sha256, hash('preferences-request'));
  assert.deepEqual(context.faultActions, ['arm', 'wait', 'disarm']);
  assert.deepEqual(context.reads.filter(([, path]) => path.includes('/events')), [
    ['a', '/api/v1/agents/agent/events?after=20'], ['b', '/api/v1/agents/agent/events?after=20'],
    ['a', '/api/v1/agents/sibling/events?after=20'], ['b', '/api/v1/agents/sibling/events?after=20'],
  ]);
  assert.deepEqual(context.completions, [['sibling', 'a', false], ['agent', 'b', true], ['sibling', 'a', true]]);
});

test('runtime loss disarms the fault and rejects unbound or repeated aborts', async () => {
  for (const [input, error] of [
    [{aborts: {a: 0, b: 0}}, /runtime_loss_abort_not_observed/],
    [{aborts: {a: 1, b: 1}}, /runtime_loss_abort_count_unbound/],
    [{consumedAborts: 0}, /runtime_loss_abort_not_consumed_once/],
    [{transactionEntered: false}, /runtime_loss_transaction_not_observed/],
    [{missWindow: true}, /runtime_loss_writer_missed_transaction_window/],
    [{writerOnIngress: true}, /runtime_loss_writer_executed_on_ingress/],
    [{writerNotLogged: true}, /verified_owner_write_missing_or_duplicated/],
    [{replaceManager: true}, /runtime_loss_manager_process_changed/],
    [{repeatAbort: true}, /runtime_loss_abort_repeated_after_retry/],
    [{scheduled: {a: 0, b: 0}}, /runtime_loss_successful_publication_unattributed/],
    [{scheduled: {a: 1, b: 1}}, /runtime_loss_successful_publication_not_unique/],
  ]) {
    const context = fixture(input);
    await assert.rejects(runtimeLoss(context), error);
    assert.deepEqual(context.faultActions, ['arm', 'wait', 'disarm']);
    assert.deepEqual(context.completions, [['sibling', 'a', false]]);
  }
});

test('a new Pod alone cannot pass while the home, conversation or event custody changes', async () => {
  for (const [input, error] of [[{changeHome: true}, /runtime_loss_home_replaced/],
    [{changeConversation: true}, /runtime_loss_conversation_replaced/],
    [{loseHistory: true}, /runtime_loss_history_changed/],
    [{loseTitle: true}, /runtime_loss_competing_publication_lost/],
    [{titleSeq: 29}, /runtime_loss_generation_retirement_unbound/],
    [{postEvents: 2}, /runtime_loss_generation_retirement_unbound/],
    [{mixedGenerations: true}, /runtime_loss_partial_generation_retirement/]]) {
    await assert.rejects(runtimeLoss(fixture(input)), error);
  }
});
