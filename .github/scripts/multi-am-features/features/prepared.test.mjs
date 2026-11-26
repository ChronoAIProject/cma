import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import test from 'node:test';
import {runInNewContext} from 'node:vm';
import {preparedJourney} from '../prepared_suite.mjs';

function fixture({consumed = true, readyIds = ['candidate-a', 'candidate-b'], repeatReservation = false,
  schemas = [4, 3, 1], readyFences = [1, 1, 1, 1], startupReceipts = [1, 1, 1],
  starting = true, ready = true, effects = 1, changedOwner = false, changedIngress = false,
  admissionRequest = 'fixture-admission', repeatRequest = false} = {}) {
  const actions = [];
  const records = [];
  let ownerReads = 0;
  let topologyReads = 0;
  const create = async (am, desktop, overrides, onAgentId) => {
    assert.equal(desktop, false);
    const sandboxId = overrides?.profile ? 'cold-' + am : 'candidate-' + (repeatReservation ? 'a' : am);
    const record = {agentId: 'fixture-agent', sandboxId, operationId: 'fixture-operation', am, admissionRequest};
    records.push(record);
    actions.push('create-' + am + '-' + sandboxId);
    onAgentId?.(record);
    return record;
  };
  const adapter = {
    topology: async () => {
      topologyReads += 1;
      return {shared_database: true, coordination_enabled: true, ams: {
        a: {ready: true, instance_id: 'fixture-a', pod_uid: changedIngress && topologyReads > 1 ? 'new-pod-a' : 'fixture-pod-a'},
        b: {ready: true, instance_id: 'fixture-manager', pod_uid: 'fixture-pod'},
      }};
    },
    preparedReady: async () => readyIds,
    preparedEvidence: async ({agentId, sandboxId, operationId}) => {
      assert.equal(agentId, 'fixture-agent');
      assert.equal(operationId, 'fixture-operation');
      return {agent_ready: true, consumed: !sandboxId.startsWith('cold-') && consumed,
        installation_bound: true, schemas, ready_fences: readyFences, startup_receipts: startupReceipts};
    },
    ownerOf: async () => ({agent_id: 'fixture-agent', am: 'b', epoch: ++ownerReads > 1 && changedOwner ? 2 : 1,
      instance_id: 'fixture-manager', pod_uid: 'fixture-pod'}),
    publicationEvidence: async () => ({a: {}, b: {known_aborted_cas: {publish_connection: 1}}}),
  };
  const results = [];
  const step = async (name, operation) => { results.push({name, ...await operation()}); };
  const browser = {forIngress: async (am, run) => run({
    async openStarting(agentId) {
      assert.equal(agentId, 'fixture-agent');
      actions.push('starting-' + am);
      return starting ? 'b'.repeat(64) : null;
    },
    async sendAndRead(agentId) {
      assert.equal(agentId, 'fixture-agent');
      actions.push('reply-' + am);
      return {effects, ready_request_sha256: ready ? (repeatRequest ? 'b' : 'c').repeat(64) : null,
        send_request_sha256: 'd'.repeat(64),
        answer_sha256: 'a'.repeat(64)};
    },
  })};
  return {adapter, create, step, results, actions, records, browser,
    cleanup: async record => actions.push('cleanup-' + record.sandboxId),
    openBoth: async () => actions.push('open-both')};
}

test('cold comparison and two ingress checks join the exact prepared reservation to Ready and Send', async () => {
  const context = fixture();
  await preparedJourney(context);
  assert.deepEqual(context.results.map(row => row.name),
    ['cold_comparison_on_a', 'prepared_create_on_a', 'cold_comparison_on_b', 'prepared_create_on_b']);
  assert.deepEqual(context.actions, [
    'create-a-cold-a', 'cleanup-cold-a',
    'create-a-candidate-a', 'starting-a', 'open-both', 'reply-a', 'cleanup-candidate-a',
    'create-b-cold-b', 'cleanup-cold-b',
    'create-b-candidate-b', 'starting-b', 'open-both', 'reply-b', 'cleanup-candidate-b',
  ]);
  assert.equal(context.results[1].atomic_publication_fence_count, 4);
  assert.equal(context.results[1].startup_receipt_count, 3);
  assert.equal(context.results[1].browser_starting_read, true);
  assert.equal(context.results[1].browser_ready_read, true);
  assert.equal(context.results[1].ingress, 'a');
  assert.equal(context.results[1].owner, 'b');
  assert.equal(context.results[1].owner_epoch, 1);
  assert.equal(context.results[1].request_sha256.browser_starting, 'b'.repeat(64));
  assert.equal(context.results[1].request_sha256.browser_ready, 'c'.repeat(64));
  assert.equal(context.results[1].request_sha256.browser_send, 'd'.repeat(64));
  assert.match(context.results[1].request_sha256.admission, /^[a-f0-9]{64}$/);
  assert.deepEqual(context.results[1].publication_diagnostics.b, {known_aborted_cas: {publish_connection: 1}});
  assert.deepEqual(context.results[1].parent_schema_versions, [4, 3, 1]);
});

test('prepared coverage requires separate correlated requests and stable owner and ingress identities', async () => {
  await assert.rejects(preparedJourney(fixture({admissionRequest: ''})), /prepared_admission_ingress_unbound/);
  await assert.rejects(preparedJourney(fixture({repeatRequest: true})), /prepared_request_attribution_reused/);
  await assert.rejects(preparedJourney(fixture({changedOwner: true})), /prepared_browser_owner_changed/);
  await assert.rejects(preparedJourney(fixture({changedIngress: true})), /prepared_browser_ingress_changed/);
});

test('a real prepared hit still needs the Starting browser read and a single reply', async () => {
  await assert.rejects(preparedJourney(fixture({starting: false})), /prepared_browser_starting_read_unproven/);
  await assert.rejects(preparedJourney(fixture({ready: false})), /prepared_browser_reply_unproven/);
  await assert.rejects(preparedJourney(fixture({effects: 2})), /prepared_browser_reply_unproven/);
});

test('a prepared Ready publication fails if the startup receipts are absent or repeated', async () => {
  for (const startupReceipts of [[1, 1, 0], [1, 2, 1], [1, 1]]) {
    await assert.rejects(preparedJourney(fixture({startupReceipts})), /prepared_startup_receipt_count_wrong/);
  }
});

test('a Ready Agent cannot stand in for four committed publication fences and supported parents', async () => {
  for (const readyFences of [[1, 1, 0, 1], [1, 1, 1, 2], [1, 1, 1]]) {
    const context = fixture({readyFences});
    await assert.rejects(preparedJourney(context), /prepared_open_ready_atomic_publication_unproven/);
    assert(!context.actions.includes('open-both'));
  }
  for (const schemas of [[4, 3, 3], [4, 8, 1], [4, '3', 1]]) {
    const context = fixture({schemas});
    await assert.rejects(preparedJourney(context), /prepared_parent_schemas_unbound/);
    assert(!context.actions.includes('open-both'));
  }
});

test('the two ingresses must consume distinct reservations', async () => {
  const context = fixture({repeatReservation: true});
  await assert.rejects(preparedJourney(context), /prepared_admission_consumption_not_observed/);
  assert.equal(context.actions.filter(action => action === 'open-both').length, 1);
});

test('a ready reservation or a separately consumed reservation cannot count as a hit', async () => {
  for (const context of [fixture({consumed: false}), fixture({readyIds: ['different-candidate']})]) {
    await assert.rejects(preparedJourney(context), /prepared_admission_consumption_not_observed/);
    assert(!context.actions.includes('open-both'));
  }
});

test('the snapshot binds the reservation installation digest to the original boot and startup receipts', async () => {
  const program = await readFile(new URL('../owner/prepared_evidence.js', import.meta.url), 'utf8');
  const lineage = {agentId: 'fixture-agent', sandboxId: 'fixture-sandbox', operationId: 'fixture-operation'};
  const origin = {operation_id: lineage.operationId};
  const runtime = {runtime_id: 'fixture-runtime', runtime_version: 'fixture-image',
    boot_id: 'fixture-boot', provision_generation: 1};
  const installed = 'a'.repeat(64);
  const receipts = ['add_worktree', 'materialize_home', 'open_conversation'].map(step =>
    ({step, operation_id: 'synthetic-' + step, request_sha256: installed}));
  const owner = 'fixture-owner';
  const fields = {owner, document_schema_version: 1, agent_prepared_open_ready_fence: 1};
  const rows = {
    agents: {_id: lineage.agentId, sandbox_id: lineage.sandboxId, owner,
      workspace_id: 'fixture-workspace', worktree_id: 'fixture-worktree', status: 'ready',
      record: {origin, work: {receipts}}},
    sandboxes: {...fields, _id: lineage.sandboxId, status: 'active', record: {origin, runtime}},
    workspaces: {...fields, _id: 'fixture-workspace', sandbox_id: lineage.sandboxId, status: 'ready'},
    worktrees: {...fields, _id: 'fixture-worktree', workspace_id: 'fixture-workspace',
      sandbox_id: lineage.sandboxId, status: 'ready'},
    native_prepared_sandboxes: {...fields, _id: lineage.sandboxId, kind: 'reservation', state: 'claimed',
      record: {ownership: {owner, operation_id: lineage.operationId},
        payload: {installed_request_sha256: installed, runtime: {...runtime}}}},
  };
  const paths = (row, key) => key.split('.').reduce((current, field) => current?.[field], row);
  const read = async () => {
    let output, began = false, ended = false;
    const snapshot = Object.fromEntries(Object.entries(rows).map(([name, row]) => [name, {
      async findOne(filter) {
        assert(began);
        return Object.entries(filter).every(([key, value]) => key === '$expr' || paths(row, key) === value)
          ? row : null;
      },
    }]));
    const session = {async startTransaction() { began = true; },
      getDatabase: () => snapshot, async endSession() { ended = true; }};
    const d = {getMongo: () => ({startSession: async () => session}), getName: () => 'synthetic'};
    await runInNewContext(program, {lineage, d, print: value => { output = JSON.parse(value); }});
    assert(ended);
    return output;
  };
  const valid = await read();
  assert.equal(valid.installation_bound, true);
  assert.deepEqual(valid.startup_receipts, [1, 1, 1]);
  assert.equal(JSON.stringify(valid).includes('fixture-boot'), false);
  rows.native_prepared_sandboxes.record.payload.runtime.boot_id = 'other-boot';
  assert.equal((await read()).installation_bound, false);
  rows.native_prepared_sandboxes.record.payload.runtime.boot_id = runtime.boot_id;
  delete rows.native_prepared_sandboxes.record.payload.installed_request_sha256;
  rows.agents.record.work.prepared_installation_digest = installed;
  assert.equal((await read()).installation_bound, false);
  rows.agents.record.origin = {operation_id: 'other-operation'};
  assert.equal((await read()).agent_ready, false);
});
