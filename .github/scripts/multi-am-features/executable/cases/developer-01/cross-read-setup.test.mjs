import test from 'node:test';
import assert from 'node:assert/strict';
import {createCrossOwnerReadFactory} from './cross-read-setup.mjs';
import {digest} from './access-common.mjs';

// Exercises the real shared hierarchy helper against a stateful wire model.
// It does not provide live provider, runtime, browser or authorization acceptance.
function model(options = {}) {
  const calls = [], cleanups = [], intents = [], retained = [], observations = [];
  let root, workspace, agent, historyText, inputText, fileBody, released = 0;
  const config = {actors: {user: {subject: 'fixture-owner'}, creator: {subject: 'fixture-owner'},
    read_operator: {subject: 'fixture-reader'}, outsider: {subject: 'fixture-outsider'}}};
  const bindings = {identities: {user: {actor: 'user', subject: 'fixture-owner'}},
    userResources: {scope: 'LOCAL_DISPOSABLE_TWO_AM', database_disposable: true, single_matrix_writer: true,
      sandbox_profile: {id: 'sbp-fixture', revision: 1}, timeout_ms: 2000,
      reserve: async () => ({parent_cleanup_barrier: true,
        retainIntent: async value => intents.push(value), retainResource: async value => retained.push(value),
        release: async () => {released++; return {released: true};}}),
      finality: async ({sandboxId}) => ({sandbox_id: sandboxId, children_remaining: 0,
        pending_operations: options.pendingAfterRetirement ? 1 : 0, cleanup_complete: true}),
    }};
  const ctx = {am: 'a', row: {case_id: 'C275:ADM-080'}, phase: 'before_restart', iteration: 1,
    assert: (value, code) => assert.ok(value, code),
    defer: (label, run) => cleanups.push({label, run}),
    observe: async (label, run) => {const value = await run(); observations.push({label, value}); return value;},
    request: async args => {
      calls.push(structuredClone(args));
      const {method, path, body, actor, allowed = [200]} = args;
      const reply = (status, value) => {
        const response = {status, body: structuredClone(value), timing: {
          am: args.am ?? 'b', request_id: 'request-' + calls.length, serving: {pod_uid: 'model-pod'},
        }};
        if (!allowed.includes(status)) throw Object.assign(Error('request_status_' + status), {response});
        return response;
      };
      if (path === '/api/v1/sandboxes' && method === 'POST') {
        root = {id: 'sandbox-fixture', owner: 'fixture-owner', name: body.name, status: 'active'};
        return reply(201, root);
      }
      if (path === '/api/v1/sandboxes/sandbox-fixture') {
        if (method === 'DELETE') {
          root.status = 'retired';
          if (options.unknownRetire) throw Error('retire_ack_unknown');
          return reply(202, root);
        }
        return reply(200, root);
      }
      if (path.endsWith('/workspaces/inline')) {
        workspace = {id: 'workspace-fixture', sandbox_id: root.id, status: 'ready'};
        return reply(201, workspace);
      }
      if (path === '/api/v1/workspaces/workspace-fixture') return reply(200, workspace);
      if (path === '/api/v1/agents' && method === 'POST') {
        historyText = body.initial_message;
        agent = {id: 'agent-fixture', name: body.name, status: 'ready', sandbox: root, workspace};
        return reply(202, {id: 'lifecycle-fixture'});
      }
      if (path === '/api/v1/agent-lifecycle/lifecycle-fixture') {
        return reply(200, {id: 'lifecycle-fixture', agent_id: agent.id, status: 'succeeded'});
      }
      if (path.startsWith('/api/v1/resource-access/agent/')) {
        return reply(200, {owner: {kind: 'person', is_current_person: !!options.readerIsOwner},
          capabilities: ['view_details', 'view_history', 'view_files', ...(options.readerCanWrite ? ['interact'] : [])]});
      }
      assert.equal(args.agentId, agent.id, 'every Agent route retains live-owner attribution');
      if (path === '/api/v1/agents/agent-fixture') {
        return actor === 'outsider' ? reply(options.outsiderAllowed ? 200 : 403, {}) : reply(200, agent);
      }
      if (path.endsWith('/transcript')) {
        const turns = [{owner_message: historyText, messages: [{role: 'assistant', content: 'OK'}],
          outcome: {type: 'completed'}}];
        if (inputText) {
          const marker = inputText.split(': ').at(-1);
          const turn = {owner_message: inputText, messages: [{role: 'assistant',
            content: options.wrongMarker ? 'Different answer' : marker}],
          outcome: {type: options.failedMarker ? 'failed' : 'completed'}};
          turns.push(turn); if (options.duplicateTurn) turns.push(turn);
        }
        return reply(200, {agent_id: agent.id, turns});
      }
      if (path.endsWith('/input')) {
        inputText = body.text;
        if (options.unknownInput) throw Error('input_ack_unknown');
        return reply(200, options.badInputReceipt ? {} : {queued: false});
      }
      if (path.endsWith('/files/tree')) return reply(200, {entries: []});
      if (path.endsWith('/files') && method === 'POST') {
        fileBody = body;
        if (options.unknownFile) throw Error('file_ack_unknown');
        return reply(200, {path: options.badFileReceipt ? 'another-file' : body.path, deleted: false});
      }
      if (path.includes('/files/content?')) return reply(200, {path: fileBody.path,
        content_base64: fileBody.content_base64,
        sha256: options.badFileReadback ? '0'.repeat(64) : digest(Buffer.from(fileBody.content_base64, 'base64'))});
      throw Error('unexpected_model_route');
    }};
  return {ctx, bindings, config, calls, cleanups, observations, intents, retained,
    get released() {return released;}, get root() {return root;},
    setup: () => createCrossOwnerReadFactory(bindings)(ctx, {config}),
    async cleanup() {
      const failures = [];
      for (const {run} of cleanups.toReversed()) {try {await run();} catch (error) {failures.push(error);}}
      return failures;
    }};
}

const failures = error => [error.message, ...(error.errors ?? []).map(failures)].join(' ');

test('cross-owner recipe creates a real public hierarchy and independently chosen transcript/file data', async () => {
  const h = model(), f = await h.setup();
  assert.equal(f.ownerSubject, 'fixture-owner'); assert.equal(f.operatorSubject, 'fixture-reader');
  assert.equal(f.agentId, 'agent-fixture');
  assert.match(f.transcriptMarker, /^admin01-transcript-/); assert.match(f.deniedMarker, /^admin01-denied-/);
  assert.ok(f.deniedInput.text.includes(f.deniedMarker)); assert.ok(f.deniedKey);
  assert.ok(f.content.startsWith('Private local')); assert.ok(f.filePath.endsWith('.txt'));
  const mutations = h.calls.filter(c => c.method === 'POST');
  assert.equal(mutations.length, 5);
  assert.deepEqual(h.intents.map(i => i.kind), ['sandbox', 'workspace', 'operation']);
  assert.deepEqual(h.retained.map(i => i.kind), ['sandbox', 'workspace', 'operation', 'agent']);
  for (const suffix of ['/input', '/files']) {
    const call = mutations.find(c => c.path.endsWith(suffix));
    assert.equal(call.actor, 'creator'); assert.equal(call.am, undefined);
    assert.ok(h.observations.some(o => o.value.key === call.key));
  }
  assert.equal(h.calls.find(c => c.path.includes('/files/content?')).am, 'a');
  await f.cleanup(); await f.cleanup(); assert.deepEqual(await h.cleanup(), []);
  assert.equal(h.calls.filter(c => c.method === 'DELETE').length, 1);
  assert.equal(h.root.status, 'retired'); assert.equal(h.released, 1);
});

for (const role of ['user', 'creator', 'read_operator', 'outsider']) {
  test('missing exact role blocks before any resource creation: ' + role, async () => {
    const h = model(); delete h.config.actors[role];
    await assert.rejects(h.setup(), /admin_cross_read_exact_roles_required/);
    assert.equal(h.calls.length, 0);
  });
}

test('owner alias and resource identity must agree without credential fallback', async () => {
  const h = model(); h.config.actors.creator.subject = 'different-owner';
  await assert.rejects(h.setup(), /admin_cross_read_exact_roles_required/);
  h.config.actors.creator.subject = 'fixture-owner'; h.bindings.identities.user.subject = 'wrong-resource-owner';
  await assert.rejects(h.setup(), /admin_cross_read_resource_owner/);
  assert.equal(h.calls.length, 0);
});

test('missing native custody fails before request dispatch', async () => {
  const h = model(); delete h.bindings.userResources.finality;
  await assert.rejects(h.setup(), /user_resource_custody_required/); assert.equal(h.calls.length, 0);
});

for (const options of [{readerIsOwner: true}, {readerCanWrite: true}, {outsiderAllowed: true}]) {
  test('a wrong role cannot qualify the fixture: ' + JSON.stringify(options), async () => {
    const h = model(options); await assert.rejects(h.setup());
    assert.ok(!h.calls.some(c => c.path.endsWith('/input') || c.path.endsWith('/files')));
    assert.deepEqual(await h.cleanup(), []); assert.equal(h.released, 1);
  });
}

for (const [option, code] of [['failedMarker', 'marker_turn_failed'], ['wrongMarker', 'marker_not_exact'],
  ['duplicateTurn', 'duplicate_turn'], ['badInputReceipt', 'input_receipt'], ['badFileReceipt', 'file_receipt'],
  ['badFileReadback', 'file_readback']]) {
  test('observed setup data must match the actual public receipt: ' + option, async () => {
    const h = model({[option]: true}); await assert.rejects(h.setup(), new RegExp(code));
    assert.deepEqual(await h.cleanup(), []); assert.equal(h.released, 1);
  });
}

for (const [option, suffix] of [['unknownInput', '/input'], ['unknownFile', '/files']]) {
  test('lost setup acknowledgement is never replayed and root custody remains: ' + option, async () => {
    const h = model({[option]: true}); await assert.rejects(h.setup(), /ack_unknown/);
    assert.deepEqual(await h.cleanup(), []); assert.equal(h.released, 1);
    assert.equal(h.calls.filter(c => c.method === 'POST' && c.path.endsWith(suffix)).length, 1);
    assert.equal(h.calls.filter(c => c.method === 'DELETE').length, 1);
  });
}

for (const [option, code] of [['unknownRetire', 'retire_ack_unknown'],
  ['pendingAfterRetirement', 'user_setup_cleanup_finality']]) {
  test('cleanup failure remains visible without another retirement: ' + option, async () => {
    const h = model({[option]: true}), f = await h.setup();
    await assert.rejects(f.cleanup(), error => failures(error).includes(code));
    await assert.rejects(f.cleanup(), error => failures(error).includes(code));
    assert.equal(h.calls.filter(c => c.method === 'DELETE').length, 1); assert.equal(h.released, 0);
  });
}
