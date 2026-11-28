import {createHash} from 'node:crypto';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import test from 'node:test';
import {runInNewContext} from 'node:vm';
import {preparedCommitAcknowledgement, preparedJourney, preparedParentEnvelopes} from '../prepared_suite.mjs';
import {isPreparedSend} from '../prepared_browser.mjs';
import {hash} from '../cma/core.mjs';

test('parent-envelope cases require exact schemas and unchanged Runtime receipts on both ingresses', async () => {
  const steps = [], cleaned = [], holds = [];
  const originalRuntime = {pod_sha256: hash('synthetic-pod'), receipts: ['first', 'second', 'third']};
  let pair, malformed = false, workspaceReadFirst = false;
  const installedRevisions = [12, 9];
  const context = {
    adapter: {
      preparedReady: async () => ['fixture-home'],
      preparedRuntimeEvidence: async () => originalRuntime,
      preparedSchemaBarrier: async value => holds.push(value),
      preparedSchemaRace: async ({parentVersions, ...scope}) => {
        assert.deepEqual(scope, {action: 'inject', operationId: 'fixture-operation',
          reservations: ['fixture-home'], barrier: 'parent_envelope'});
        pair = parentVersions;
        return {agentId: 'fixture-agent', sandboxId: 'fixture-home', parentVersions,
          parentRevisions: installedRevisions};
      },
      preparedEvidence: async () => ({consumed: true, agent_ready: true, reservation_bound: true,
        installation_bound: true, schemas: malformed ? [4, 3, 1] : [...pair, 1],
        ready_fences: [1, 1, 1, 1], startup_receipts: [1, 1, 1]}),
      ownerOf: async () => ({agent_id: 'fixture-agent', am: 'a', epoch: 2, pod_uid: 'fixture-owner-pod'}),
      publicationEvidence: async () => ({a: {
        prepared_commit_work_lease_sha256: [hash('fixture-commit')],
        prepared_commit_parent_revisions: [workspaceReadFirst ? [12, 8] : installedRevisions],
      }, b: {}}),
    },
    create: async (ingress, desktop, _overrides, _open, admitted) => {
      assert.equal(desktop, false);
      const record = {agentId: 'fixture-agent', sandboxId: 'fixture-home', operationId: 'fixture-operation',
        createToReadyMs: 4000, runtime: {pod_uid: 'synthetic-pod'}, ingress};
      admitted(record);
      return record;
    },
    cleanup: async record => cleaned.push(record.ingress),
    step: async (name, run) => steps.push({name, ...await run()}),
  };
  await preparedParentEnvelopes(context);
  assert.deepEqual(steps.map(step => step.parent_schema_versions),
    [[1, 1, 1], [1, 3, 1], [3, 1, 1], [3, 3, 1], [4, 1, 1], [4, 3, 1]]);
  assert.deepEqual(steps.map(step => step.prepared_commit_parent_revisions),
    Array.from({length: 6}, () => installedRevisions));
  assert.deepEqual(cleaned, ['a', 'b', 'a', 'b', 'a', 'b']);
  assert.deepEqual(holds.slice(-2), [{arm: true, parentEnvelope: true}, {arm: false}]);
  // Final schema and fence evidence alone would accept a stale Workspace read.
  workspaceReadFirst = true;
  await assert.rejects(preparedParentEnvelopes(context), /prepared_parent_envelope_committed_revisions_mismatch/);
  workspaceReadFirst = false;
  assert.equal(cleaned.length, 6);
  malformed = true;
  await assert.rejects(preparedParentEnvelopes(context), /prepared_parent_envelope_publication_unproven/);
  assert.deepEqual(holds.at(-1), {arm: false});
});

test('prepared acknowledgement loss requires exact attribution, unique fences and unchanged Runtime receipts', async () => {
  const agentId = 'fixture-agent', sandboxId = 'fixture-home', operationId = 'fixture-operation';
  const record = {agentId, sandboxId, operationId, runtime: {pod_uid: 'fixture-runtime'}};
  const commitProof = {transaction: hash('fixture-transaction'), baseline: 12};
  const runtime = {pod_sha256: hash('fixture-runtime'), receipts: ['first', 'second', 'third']};
  const configs = [], cleanups = [], opens = [];
  let diagnostics = {a: {unknown_commit: {publish_connection: 1}}, b: {}};
  let fences = [1, 1, 1, 1], readback = runtime, lostCommits = 5;
  const context = {
    adapter: {
      preparedReady: async () => [sandboxId],
      preparedRuntimeEvidence: async request => request.operationId ? readback : runtime,
      preparedSchemaBarrier: async config => configs.push(config),
      async preparedSchemaRace({action, ...scope}) {
        const common = {operationId, reservations: [sandboxId], barrier: 'commit_acknowledgement'};
        if (action === 'arm_commit') {
          assert.deepEqual(scope, common);
          return {agentId, sandboxId, receipt_count: 2, commitProof};
        }
        assert.equal(action, 'verify_commit');
        assert.deepEqual(scope, {...common, commitProof});
        return {commitAcknowledgementsLost: lostCommits};
      },
      ownerOf: async () => ({agent_id: agentId, am: 'a', epoch: 2, pod_uid: 'fixture-owner-pod'}),
      publicationEvidence: async () => diagnostics,
      preparedEvidence: async () => ({consumed: true, agent_ready: true, reservation_bound: true,
        installation_bound: true, ready_fences: fences, startup_receipts: [1, 1, 1]}),
    },
    create: async (_, __, ___, ____, onAdmission) => { onAdmission(record); return record; },
    openBoth: async subject => opens.push(subject),
    cleanup: async subject => cleanups.push(subject),
  };
  const output = await preparedCommitAcknowledgement(context);
  assert.equal(output.commit_acknowledgements_lost, 5);
  assert.equal(output.runtime_receipt_count, 3);
  assert.deepEqual(opens, [record]);
  assert.deepEqual(cleanups, [record]);

  diagnostics.a.prepared_commit_work_lease_sha256 = [hash('another-transaction')];
  await assert.rejects(preparedCommitAcknowledgement(context), /prepared_ack_outcome_unproven/);
  diagnostics = {a: {unknown_commit: {publish_connection: 1}}, b: {unknown_commit: {publish_connection: 1}}};
  await assert.rejects(preparedCommitAcknowledgement(context), /prepared_ack_outcome_unproven/);
  diagnostics.b = {};
  lostCommits = 10;
  await assert.rejects(preparedCommitAcknowledgement(context), /prepared_ack_fault_budget_unproven/);
  lostCommits = 5;
  fences = [1, 2, 1, 1];
  await assert.rejects(preparedCommitAcknowledgement(context), /prepared_ack_durable_commit_unproven/);
  fences = [1, 1, 1, 1];
  readback = {...runtime, receipts: ['changed', 'second', 'third']};
  await assert.rejects(preparedCommitAcknowledgement(context), /prepared_ack_runtime_receipts_changed/);
  assert.deepEqual(configs.at(-1), {arm: false, commitAcknowledgement: true});
  assert.deepEqual(cleanups, [record]);
});

test('Mongo acknowledgement verification refuses equal log counts after a mixed transaction exhausts the fault', async () => {
  const script = await readFile(new URL('../owner/prepared_schema_race.js', import.meta.url), 'utf8');
  const app = 'synthetic-manager';
  const command = (id, affected) => JSON.stringify({
    msg: 'Slow query',
    attr: {appName: app, command: {commitTransaction: 1, lsid: {id}, txnNumber: 2},
      ...(affected ? {writeConcernError: {code: 50}} : {})},
  });
  const target = 'fixture-target', other = 'fixture-unrelated', baseline = 12;
  const scope = {action: 'verify_commit', app, commitProof: {transaction: hash(target + ':2'), baseline}};
  const targetErrors = Array.from({length: 5}, () => command(target, true));
  const verify = async (affected, log) => {
    let result;
    await runInNewContext(script, {scope, d: {}, EJSON: JSON,
      require: name => { assert.equal(name, 'crypto'); return {createHash}; },
      admin: {runCommand: request => request.getLog ? {ok: 1, log} : {ok: 1, count: baseline + affected}},
      print: line => { result = JSON.parse(line); }});
    return result;
  };
  assert.deepEqual(await verify(5, targetErrors), {commitAcknowledgementsLost: 5});
  await assert.rejects(verify(10, [...targetErrors,
    ...Array.from({length: 5}, () => command(other, true)),
    ...Array.from({length: 5}, () => command(target, false))]), /prepared_ack_fault_budget_unproven/);
});

function fixture({consumed = true, readyIds = ['candidate-a', 'candidate-b', 'candidate-c'], repeatReservation = false,
  schemas = [4, 3, 1], readyFences = [1, 1, 1, 1], startupReceipts = [1, 1, 1],
  starting = true, ready = true, effects = 1, changedOwner = false, changedIngress = false,
  admissionRequest = 'fixture-admission', repeatRequest = false, collisionMatches = true, observed = 1,
  failCreation = false, abortManager = 'b', createToReadyMs = 5000, reservationBound = true,
  changedNativeReceipt = false, collisionOwner = 'fixture-manager', collisionEpoch = 1,
  expiredCollisionLease = false, committedWorkLeases = [hash('1:fixture-worker')], agentUnchanged = true} = {}) {
  const actions = [];
  const records = [];
  let ownerReads = 0;
  let topologyReads = 0;
  const create = async (am, desktop, overrides, onAgentId, onAdmission) => {
    assert.equal(desktop, false);
    const candidate = records.length >= 4 ? 'c' : am;
    const sandboxId = overrides?.profile ? 'cold-' + am : 'candidate-' + (repeatReservation ? 'a' : candidate);
    const record = {agentId: 'fixture-agent', sandboxId, operationId: 'fixture-operation', am, admissionRequest,
      createToReadyMs, readyObservedAt: Date.now(), runtime: {pod_uid: 'fixture-runtime-pod-' + sandboxId}};
    records.push(record);
    actions.push('create-' + am + '-' + sandboxId);
    onAdmission?.(record);
    if (failCreation && am === 'b' && onAgentId) throw new Error('fixture_admission_failed');
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
    preparedRuntimeEvidence: async ({sandboxId, operationId}) => {
      assert(readyIds.includes(sandboxId));
      if (operationId) assert.equal(operationId, 'fixture-operation');
      const changed = operationId && changedNativeReceipt && sandboxId === 'candidate-b';
      return {format: 'indexed', pod_sha256: hash('fixture-runtime-pod-' + sandboxId),
        receipts: ['0', '1', changed ? '3' : '2'].map(digit => digit.repeat(64))};
    },
    preparedEvidence: async ({agentId, sandboxId, operationId}) => {
      assert.equal(agentId, 'fixture-agent');
      assert.equal(operationId, 'fixture-operation');
      return {agent_ready: true, consumed: !sandboxId.startsWith('cold-') && consumed,
        installation_bound: true, reservation_bound: reservationBound,
        schemas, ready_fences: readyFences, startup_receipts: startupReceipts};
    },
    ownerOf: async () => ({agent_id: 'fixture-agent', am: 'b', epoch: ++ownerReads > 1 && changedOwner ? 2 : 1,
      instance_id: 'fixture-manager', pod_uid: 'fixture-pod'}),
    publicationEvidence: async () => {
      const diagnostics = {a: {}, b: {prepared_commit_work_lease_sha256: committedWorkLeases}};
      diagnostics[abortManager].known_aborted_cas = {publish_connection: observed};
      return diagnostics;
    },
    preparedSchemaBarrier: async ({arm, ownerFence, reservationConflict}) => {
      assert.equal(Boolean(ownerFence) !== Boolean(reservationConflict), true);
      actions.push(arm ? (ownerFence ? 'arm-owner-fence-hold' : 'arm-worktree-hold') : 'disarm-worktree-hold');
    },
    preparedSchemaRace: async ({action, operationId, reservations, barrier}) => {
      assert.equal(operationId, 'fixture-operation');
      assert.deepEqual(reservations, readyIds);
      assert.equal(barrier, records.length >= 5 ? 'owner_fence' : 'publication_conflict');
      actions.push(action + (barrier === 'owner_fence' ? '-owner-marker' : '-reservation-marker'));
      const sandboxId = collisionMatches ? records.at(-1).sandboxId : 'different-reservation';
      return action === 'inject' ? {agentId: 'fixture-agent', sandboxId,
        ownerInstance: collisionOwner, ownerEpoch: collisionEpoch,
        leaseDeadlineMs: Date.now() + (expiredCollisionLease ? -1000 : 60000),
        workLeaseSha256: hash('1:fixture-worker'), agentUnchanged} : {restored: 1};
    },
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
        answer_sha256: 'a'.repeat(64), transport: 'responses_v2'};
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
    ['cold_comparison_on_a', 'prepared_create_on_a', 'cold_comparison_on_b', 'prepared_create_on_b',
      'prepared_owner_fence_conflict']);
  assert.deepEqual(context.actions, [
    'create-a-cold-a', 'cleanup-cold-a',
    'create-a-candidate-a', 'starting-a', 'open-both', 'reply-a', 'cleanup-candidate-a',
    'create-b-cold-b', 'cleanup-cold-b',
    'arm-worktree-hold', 'create-b-candidate-b', 'inject-reservation-marker', 'starting-b',
    'disarm-worktree-hold', 'restore-reservation-marker',
    'open-both', 'reply-b', 'cleanup-candidate-b',
    'arm-owner-fence-hold', 'create-a-candidate-c', 'inject-owner-marker', 'starting-a',
    'disarm-worktree-hold', 'restore-owner-marker',
    'open-both', 'reply-a', 'cleanup-candidate-c',
  ]);
  assert.equal(context.results[1].atomic_publication_fence_count, 4);
  assert.equal(context.results[1].startup_receipt_count, 3);
  assert.equal(context.results[1].browser_starting_read, true);
  assert.equal(context.results[1].browser_ready_read, true);
  assert.equal(context.results[1].browser_send_transport, 'responses_v2');
  assert.equal(context.results[1].ingress, 'a');
  assert.equal(context.results[1].owner, 'b');
  assert.equal(context.results[1].owner_epoch, 1);
  assert.equal(context.results[1].request_sha256.browser_starting, 'b'.repeat(64));
  assert.equal(context.results[1].request_sha256.browser_ready, 'c'.repeat(64));
  assert.equal(context.results[1].request_sha256.browser_send, 'd'.repeat(64));
  assert.match(context.results[1].request_sha256.admission, /^[a-f0-9]{64}$/);
  assert.deepEqual(context.results[1].publication_diagnostics.b, {known_aborted_cas: {publish_connection: 1},
    prepared_commit_work_lease_sha256: [hash('1:fixture-worker')]});
  assert.equal(context.results[1].observed_known_abort, false);
  assert.equal(context.results[3].observed_known_abort, true);
  assert.equal(context.results[3].ready_before_collision_lease_deadline, true);
  assert.equal(context.results[3].runtime_receipt_format, 'indexed');
  assert.equal(context.results[3].runtime_receipt_count, 3);
  assert.equal(context.results[3].create_to_ready_ms, 5000);
  assert.deepEqual(context.results[1].parent_schema_versions, [4, 3, 1]);
  assert.equal(context.results[4].owner_fence_agent_unchanged, true);
  assert.equal(context.results[4].observed_known_abort, true);
  await assert.rejects(preparedJourney(fixture({agentUnchanged: false})), /prepared_owner_fence_agent_changed/);
});

test('prepared publication binds the collision to admission and retains the owner attribution', async () => {
  await assert.rejects(preparedJourney(fixture({collisionMatches: false})),
    /prepared_publication_collision_lineage_unbound/);
  await assert.rejects(preparedJourney(fixture({observed: 0})), /prepared_publication_abort_not_attributed/);
  const wrongManager = fixture({abortManager: 'a'});
  const diagnostics = [];
  await assert.rejects(preparedJourney({...wrongManager, recordAbortEvidence: row => diagnostics.push(row)}),
    /prepared_publication_abort_not_attributed/);
  assert.deepEqual(diagnostics, [{ingress: 'b', owner: 'b', owner_epoch: 1,
    owner_instance_sha256: hash('fixture-manager'), agent_sha256: hash('fixture-agent'),
    lifecycle_operation_sha256: hash('fixture-operation'), exact_reservation_conflict: true,
    collision_owner_epoch: 1, ready_before_collision_lease_deadline: true,
    committed_with_collision_work_lease: true,
    publication_diagnostics: {a: {known_aborted_cas: {publish_connection: 1}},
      b: {prepared_commit_work_lease_sha256: [hash('1:fixture-worker')]}}}]);
  const failed = fixture({failCreation: true});
  await assert.rejects(preparedJourney(failed), /fixture_admission_failed/);
  assert.deepEqual(failed.actions.slice(-2), ['disarm-worktree-hold', 'restore-reservation-marker']);
});

test('prepared Ready requires the collision owner and precedes its observed work lease deadline', async () => {
  await assert.rejects(preparedJourney(fixture({collisionOwner: 'fixture-other-member'})),
    /prepared_publication_owner_changed_since_collision/);
  await assert.rejects(preparedJourney(fixture({collisionEpoch: 2})),
    /prepared_publication_owner_changed_since_collision/);
  await assert.rejects(preparedJourney(fixture({expiredCollisionLease: true})),
    /prepared_publication_original_work_lease_elapsed/);
  for (const committedWorkLeases of [[], [hash('2:fixture-worker')], [hash('1:other-worker')]]) {
    await assert.rejects(preparedJourney(fixture({committedWorkLeases})),
      /prepared_publication_work_holder_or_fence_changed/);
  }
});

test('prepared publication retry must preserve the Runtime receipts present before the claim', async () => {
  await assert.rejects(preparedJourney(fixture({changedNativeReceipt: true})), /prepared_native_mutation_receipts_changed/);
});

test('prepared Ready must be observed before the original work lease could expire', async () => {
  for (const createToReadyMs of [null, 0, 18 * 60 * 1000]) {
    await assert.rejects(preparedJourney(fixture({createToReadyMs})), /prepared_ready_work_lease_bound_unproven/);
  }
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
  for (const schemas of [[4, 3, 3], [4, 4, 1], [4, 8, 1], [4, '3', 1]]) {
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
  for (const context of [fixture({consumed: false}), fixture({readyIds: ['different-candidate']}),
    fixture({reservationBound: false})]) {
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
  const receipts = [
    ['add_worktree', 'worktree:workspace:wt-1:add:v1'],
    ['materialize_home', `agent:${lineage.agentId}:home:create:v1`],
    ['open_conversation', `agent:${lineage.agentId}:open:v1`],
  ].map(([step, operation_id]) => ({step, operation_id, request_sha256: installed}));
  const owner = 'fixture-owner';
  const fields = {owner, document_schema_version: 1, agent_prepared_open_ready_fence: 1};
  const rows = {
    agents: {_id: lineage.agentId, sandbox_id: lineage.sandboxId, owner,
      workspace_id: 'fixture-workspace', worktree_id: 'fixture-worktree', status: 'ready',
      record: {origin, work: {receipts}, conversation: {conversation_id: 'fixture-conversation'}}},
    sandboxes: {...fields, _id: lineage.sandboxId, status: 'active', record: {origin, runtime}},
    workspaces: {...fields, _id: 'fixture-workspace', sandbox_id: lineage.sandboxId, status: 'ready'},
    worktrees: {...fields, _id: 'fixture-worktree', workspace_id: 'fixture-workspace',
      sandbox_id: lineage.sandboxId, status: 'ready'},
    native_prepared_sandboxes: {...fields, _id: lineage.sandboxId, kind: 'reservation', state: 'claimed',
      record: {ownership: {owner, operation_id: lineage.operationId},
        payload: {installed_request_sha256: installed, runtime: {...runtime}, workspaces: [
          {id: 'fixture-workspace', agents: [{id: lineage.agentId,
            worktree_id: 'fixture-worktree', conversation_id: 'fixture-conversation'}]},
        ]}}},
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
  assert.equal(valid.reservation_bound, true);
  assert.deepEqual(valid.startup_receipts, [1, 1, 1]);
  assert.equal(JSON.stringify(valid).includes('fixture-boot'), false);
  receipts[2].operation_id = 'agent:another-agent:open:v1';
  assert.deepEqual((await read()).startup_receipts, [1, 1, 0]);
  receipts[2].operation_id = `agent:${lineage.agentId}:open:v1`;
  receipts.push({...receipts[2]});
  assert.deepEqual((await read()).startup_receipts, [1, 1, 2]);
  receipts.pop();
  const prepared = rows.native_prepared_sandboxes.record.payload.workspaces[0].agents[0];
  prepared.conversation_id = 'other-conversation';
  assert.equal((await read()).reservation_bound, false);
  prepared.conversation_id = 'fixture-conversation';
  prepared.worktree_id = 'other-worktree';
  assert.equal((await read()).reservation_bound, false);
  rows.native_prepared_sandboxes.record.payload.runtime.boot_id = 'other-boot';
  assert.equal((await read()).installation_bound, false);
  rows.native_prepared_sandboxes.record.payload.runtime.boot_id = runtime.boot_id;
  delete rows.native_prepared_sandboxes.record.payload.installed_request_sha256;
  rows.agents.record.work.prepared_installation_digest = installed;
  assert.equal((await read()).installation_bound, false);
  rows.agents.record.origin = {operation_id: 'other-operation'};
  assert.equal((await read()).agent_ready, false);
});
test('prepared browser observes the exact message on both public Send transports', () => {
  const request = (path, body, method = 'POST') => ({method: () => method,
    url: () => 'http://127.0.0.1:1234' + path, postDataJSON: () => body});
  const v2 = '/api/v2/agents/agt_fixture/responses';
  const v1 = '/api/v1/agui/run';
  const input = 'Original fixture message';
  const agui = {threadId: 'agt_fixture', messages: [{role: 'user', content: input}]};
  for (const [value, expected] of [
    [request(v2, {input, stream: true}), true],
    [request(v1, agui), true],
    [request(v2 + '/receipt', {input, stream: true}), false],
    [request(v2, {input, stream: true}, 'GET'), false],
    [request(v2, {input: 'Changed', stream: true}), false],
    [request(v2, {input, stream: false}), false],
    [request(v1, {...agui, threadId: 'agt_other'}), false],
  ]) assert.equal(isPreparedSend(value, 'agt_fixture', input), expected);
});
