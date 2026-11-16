import test from 'node:test';
import assert from 'node:assert/strict';
import {installContext} from './context.mjs';
import {requireOwnerProof} from './owner-proof.mjs';
import {digest} from './provenance.mjs';
import {j5, deletedAgent} from './transitions.mjs';
import {j3, exactTurn} from './turns.mjs';
import {signer, subject} from './test-support.mjs';
import {Blocked} from '../../executable/runner.mjs';
import {cleanupAgent} from './cleanup.mjs';

function proof(receipt, agentId = 'unit-agent') {
  return {verdict: 'PASS', request_id: receipt.request_id, agent_sha256: digest(agentId),
    owner: 'a', ingress: 'b', kind: 'non_owner', owner_epoch: 9,
    owner_instance_sha256: digest('a'), ingress_instance_sha256: digest('b')};
}
function context(state, operation) {
  const signed = signer(), observations = [], cleanups = [], receipts = new Map();
  const parent = {proofs: [], lifecycleTrust: signed.trust,
    adapter: {ownerOf: async () => ({am: 'a'})}};
  let sequence = 0;
  const ctx = {remainingMs: () => 10000,
    assert: (condition, code) => assert.ok(condition, code),
    defer: (label, run) => cleanups.push({label, run}),
    observe: async (label, run) => {const value = await run(); observations.push({label, value}); return value;},
    fixture: name => name === 'lifecycle-evidence' ? {collect: async ({request_id}) => {
      const receipt = receipts.get(request_id);
      return signed.capture({schema: 'mfunc01-lifecycle-owner-v1', challenge: request_id,
        ...proof({request_id}, state.agentId), kind: 'lifecycle', method: receipt.method,
        path_sha256: digest(receipt.path), status: 202, ingress: receipt.am});
    }} : null,
    request: async input => {
      const am = input.am ?? 'b', request_id = 'unit-request-' + (++sequence);
      const result = await operation(input);
      const response = {status: result.status ?? (input.method === 'GET' ? 200 : 202),
        body: result.body ?? {}, timing: {am, request_id}};
      receipts.set(request_id, {...input, am});
      if (input.agentId) parent.proofs.push(proof({request_id}, input.agentId));
      return response;
    }};
  installContext(ctx, parent);
  return {ctx, parent, observations, cleanups, receipts};
}
const state = () => ({agentId: 'unit-agent', sandboxId: 'unit-sandbox', am: 'a', key: 'unit-create',
  identity: {actor: 'new-user', registration: {subject_sha256: digest(subject)}}});

test('every mutation must have its own matching epoch/request/Agent proof', async () => {
  const s = state(); s.requestIds = [1, 2].map(n => ({request_id: 'r' + n, method: 'PATCH', am: 'b'}));
  const {ctx, parent} = context(s, async () => ({}));
  parent.proofs.push(proof(s.requestIds[0]));
  await assert.rejects(requireOwnerProof(ctx, s, 'unit_owner'), /owner_proof_missing/);
  parent.proofs.push(proof(s.requestIds[1]));
  assert.equal((await requireOwnerProof(ctx, s, 'unit_owner')).length, 2);
  for (const update of [{agent_sha256: digest('another-agent')}, {owner_epoch: 0}, {ingress: 'a'}, {verdict: 'FAIL'}]) {
    parent.proofs[1] = {...proof(s.requestIds[1]), ...update};
    await assert.rejects(requireOwnerProof(ctx, s, 'unit_owner'), error => error instanceof Blocked);
  }
});

test('J5 does not pause while Agent is stopping, records terminal reads on opposite ingress', async () => {
  const s = state(), events = []; let reads = 0, phase = 'working';
  const {ctx, observations} = context(s, async input => {
    events.push(input.method + ' ' + input.path);
    if (input.path.endsWith('/stop')) {assert.deepEqual(input.body, {reason: 'owner'}); phase = 'stopping';}
    else if (input.method === 'GET' && input.path.endsWith('/unit-agent')) {
      reads++; if (reads > 1) phase = 'stopped'; return {body: {status: phase}};
    } else if (input.path.endsWith('/pause')) {assert.equal(phase, 'stopped'); phase = 'paused';}
    else if (input.path.endsWith('/revive')) phase = 'active';
    return {body: {id: s.sandboxId, status: phase}};
  });
  await j5(ctx, s);
  assert.equal(reads, 2);
  assert.ok(events.indexOf('POST /api/v1/sandboxes/unit-sandbox/pause') > 2);
  for (const label of ['stop_terminal_peer_read', 'pause_terminal_peer_read', 'resume_terminal_peer_read']) {
    const receipt = observations.find(e => e.label === label).value;
    assert.notEqual(receipt.mutation_am, receipt.terminal_am);
    assert.ok(receipt.mutation_request_id && receipt.terminal_request_id);
  }
});

test('terminal delete accepts complete tombstone or absence, never intermediate status', () => {
  assert.equal(deletedAgent({status: 200, body: {status: 'deleting'}}), false);
  assert.equal(deletedAgent({status: 200, body: {status: 'deleted', worktree: {status: 'removing'}}}), false);
  assert.equal(deletedAgent({status: 200, body: {status: 'deleted', worktree: {status: 'removed'}}}), true);
  assert.equal(deletedAgent({status: 404}), true);
});

test('exact turn cannot borrow another completed reply or interrupt', () => {
  const value = {body: {turns: [{owner_message: 'previous', outcome: {type: 'completed'}, agent_messages: ['OK']}]}};
  assert.equal(exactTurn(value, 'current', 'completed'), null);
  value.body.next = {turn: 2};
  assert.throws(() => exactTurn(value, 'current', 'completed'), /incomplete/);
});

test('J3 completes exact OK first; observes a second active turn before interrupt and releases afterward', async () => {
  const s = state(), turns = [], actions = []; let heldId;
  const {ctx, parent} = context(s, async input => {
    if (input.path.endsWith('/transcript')) return {body: {turns}};
    if (input.path.endsWith('/input')) {
      actions.push('send');
      turns.push({turn_sequence: turns.length + 1, owner_message: input.body.text,
        outcome: {type: turns.length ? 'active' : 'completed'}, agent_messages: turns.length ? [] : ['OK']});
    } else if (input.path.endsWith('/interrupt')) {
      assert.equal(turns[1].outcome.type, 'active'); actions.push('interrupt'); turns[1].outcome.type = 'interrupted';
    }
    return {};
  });
  const original = ctx.fixture;
  ctx.fixture = name => name === 'runtime-controls' ? {
    prepareTurn: async ({caseId}) => {actions.push('hold'); heldId = caseId;},
    oracle: async () => {actions.push('active'); return {case_id: heldId, request_count: 1, active_streams: 1, completed_effect_count: 0};},
    releaseTurn: async () => {assert.equal(turns[1].outcome.type, 'interrupted'); actions.push('release');},
  } : original(name);
  await j3(ctx, s);
  assert.deepEqual(actions, ['send', 'hold', 'send', 'active', 'interrupt', 'release']);
});

test('cleanup reconciles an uncertain delete without replaying it and retires the Sandbox', async () => {
  const s = {...state(), deleteAttempted: true}, writes = [];
  const {ctx, parent} = context(s, async input => {
    if (input.method !== 'GET') {writes.push(input); return {};}
    if (input.path.endsWith('/unit-agent')) return {status: 404};
    return {body: {status: writes.length ? 'retired' : 'active'}};
  });
  await cleanupAgent(ctx, s);
  assert.deepEqual(writes.map(v => v.path), ['/api/v1/sandboxes/unit-sandbox']);
  assert.equal(parent.resourcesReleased, true);
});

test('unknown create fails cleanup without any new mutation', async () => {
  const s = {...state(), unresolvedCreate: true}; let calls = 0;
  const {ctx} = context(s, async () => {calls++; return {};});
  await assert.rejects(cleanupAgent(ctx, s), /unknown_create/); assert.equal(calls, 0);
});
