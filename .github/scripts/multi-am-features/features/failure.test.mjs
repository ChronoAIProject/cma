import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp, readdir, readFile, rm, stat} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {createHash} from 'node:crypto';
import {runInNewContext} from 'node:vm';
import {journeyFailureSummary, lifecycleFailureSummary, lifecyclePollFailure, provenanceRequestFailure,
  retainFailure, withRestoration} from '../failure.mjs';
import {NonOwnerRouter} from '../routing.mjs';
import {pollLifecycleOperation} from '../cma/lifecycle_operation.mjs';

test('hosted summaries keep the initiating failure and count cleanup failures without resource identities', () => {
  const result = journeyFailureSummary({
    steps: [{name: 'initialization', verdict: 'PASS'},
      {name: 'prepared_create_on_a', verdict: 'FAIL', error: 'prepared_parent_schemas_unbound'}],
    create_failure: {observation: lifecycleFailureSummary({status: 'running', phase: 'workspace_runtime',
      agent_id: 'private-agent', error: 'private fixture detail'}),
    poll_failure: lifecyclePollFailure(new TypeError('fetch failed',
      {cause: Object.assign(Error('private socket'), {code: 'ECONNRESET'})}), 'b')},
    failure_capture: {runtime_pod_count: 0, path: '/private/capture'},
    cleanup: [{public_cleanup: 'FAILED', reason: 'private fixture detail', agent_sha256: 'private-agent'},
      {am_restore: 'FAILED'}, {public_cleanup: 'PASS'}],
  });
  assert.deepEqual(result, {first_failed_step: 'prepared_create_on_a',
    first_failed_step_code: 'prepared_parent_schemas_unbound',
    lifecycle_failure: {status: 'running', phase: 'workspace_runtime', agent_assigned: true},
    lifecycle_poll_failure: {manager: 'b', kind: 'transport_error', cause: 'ECONNRESET'},
    failure_runtime_pod_count: 0,
    journey_cleanup_failures: {public_retirement: 1, provider_release: 0, manager_restore: 1}});
  assert.equal(journeyFailureSummary({steps: [{name: 'initialization', verdict: 'FAIL', error: '/private/file'}]})
    .first_failed_step_code, 'transport_or_contract_error');
  assert.deepEqual(lifecycleFailureSummary({status: 'private-state', phase: 'private-phase'}),
    {status: 'unknown', phase: 'unknown', agent_assigned: false});
});

test('lifecycle polling publishes only recognized failure types and status', () => {
  const http = Object.assign(Error('private response'), {response: {status: 503, body: {code: 'private'}}});
  assert.deepEqual(lifecyclePollFailure(http, 'a'), {manager: 'a', kind: 'http_error', status: 503});
  assert.deepEqual(lifecyclePollFailure(new DOMException('private', 'TimeoutError'), 'b'),
    {manager: 'b', kind: 'request_timeout'});
  const unknown = new TypeError('private body', {cause: {code: 'private_cause'}});
  assert.deepEqual(lifecyclePollFailure(unknown, 'b'), {manager: 'b', kind: 'unknown'});
});

test('provenance failure retains only the known request metadata and problem code', () => {
  const response = {status: 503, timing: {am: 'b', method: 'PATCH',
    route: '/api/v1/agents/{id}/preferences', request_id: 'private'}, body: {
    code: 'agent_owner_unavailable', detail: 'private details',
  }};
  const requestFailure = provenanceRequestFailure(response);
  const summary = journeyFailureSummary({steps: [{name: 'provenance_progress_known_abort',
    verdict: 'FAIL', error: 'api_status_503', provenance_request_failure: requestFailure}]});
  assert.deepEqual(summary.provenance_request_failure, {
    manager: 'b', method: 'PATCH', route: '/api/v1/agents/{id}/preferences', status: 503,
    problem_code: 'agent_owner_unavailable',
  });
  assert.equal(provenanceRequestFailure({...response, body: {code: 'private'}}).problem_code, undefined);
  assert.equal(provenanceRequestFailure({...response, timing: {...response.timing,
    route: '/api/v1/agents/private/preferences'}}), undefined);
  assert.equal(provenanceRequestFailure({...response, status: 200}), undefined);
});

test('the two-Manager read summary identifies the transcript transport failure', () => {
  const error = new TypeError('private request URL', {cause: {code: 'UND_ERR_SOCKET'}});
  const result = journeyFailureSummary({
    steps: [{name: 'prepared_create_on_b', verdict: 'FAIL', error: 'prepared_publication_both_managers_read_failed'}],
    open_failure: {stage: 'agent_transcript', am: 'a', request_failure: lifecyclePollFailure(error, 'a'),
      agent_sha256: 'a'.repeat(64), failure_class: 'private detail', native_response_path: '/private/response'},
  });
  assert.deepEqual(result.open_failure, {
    stage: 'agent_transcript', manager: 'a', kind: 'transport_error', cause: 'UND_ERR_SOCKET',
  });
});

test('lifecycle polling recovers one socket failure and requires the settled operation', async () => {
  const events = [new TypeError('fetch failed', {cause: {code: 'UND_ERR_SOCKET'}}), {status: 'running'},
    {status: 'succeeded'}];
  const transient = [];
  let forwardReopened = false;
  const result = await pollLifecycleOperation(() => {
    const event = events.shift();
    if (event instanceof Error) throw event;
    assert.equal(forwardReopened, true);
    return event;
  }, body => body.status === 'succeeded', {manager: 'a', interval: 0, timeout: 1000,
    onTransient: async failure => {
      transient.push(failure);
      await new Promise(resolve => setTimeout(resolve, 10));
      forwardReopened = true;
    }});
  assert.deepEqual(result, {status: 'succeeded'});
  assert.deepEqual(transient, [{manager: 'a', kind: 'transport_error', cause: 'UND_ERR_SOCKET'}]);
});

test('lifecycle polling fails on repeated transport errors or an HTTP refusal', async () => {
  const first = new DOMException('private outcome', 'TimeoutError');
  const terminal = new TypeError('fetch failed', {cause: {code: 'UND_ERR_SOCKET'}});
  const refused = Object.assign(Error('private response'), {response: {status: 503}});
  const transient = [];
  const options = {manager: 'b', interval: 0, timeout: 1000, onTransient: failure => transient.push(failure)};
  for (const [failures, expected] of [[[first, terminal], terminal], [[refused], refused]]) {
    await assert.rejects(pollLifecycleOperation(() => { throw failures.shift(); }, () => false, options),
      error => error === expected);
    assert.deepEqual(failures, []);
  }
  assert.deepEqual(transient, [{manager: 'b', kind: 'request_timeout'}]);
});

test('a recovered lifecycle socket cannot extend the original readiness deadline', async t => {
  let elapsed = 0, reads = 0;
  const transient = [];
  t.mock.method(performance, 'now', () => elapsed);
  await assert.rejects(pollLifecycleOperation(async () => {
    reads += 1;
    if (reads === 1) throw new TypeError('fetch failed', {cause: {code: 'UND_ERR_SOCKET'}});
    assert.equal(reads, 2, 'readiness must not poll again after its original deadline');
    elapsed += 60;
    return {status: 'running', phase: 'agent_startup'};
  }, body => body.status === 'succeeded', {manager: 'b', interval: 0, timeout: 100,
    code: 'fixture_ready_deadline', onTransient: failure => {
      transient.push(failure);
      elapsed += 60;
    }}), /fixture_ready_deadline/);
  assert.equal(reads, 2);
  assert.deepEqual(transient, [{manager: 'b', kind: 'transport_error', cause: 'UND_ERR_SOCKET'}]);
});

test('the failure snapshot binds parent progress to the exact admitted Agent and owner', async () => {
  const source = await readFile(new URL('../owner/creation_failure_snapshot.js', import.meta.url), 'utf8');
  const owner = 'fixture-owner';
  const operationId = 'fixture-operation';
  const records = {
    agent_lifecycle_operations: {_id: operationId, owner, sandbox_id: 'fixture-sandbox', agent_id: 'fixture-agent'},
    agents: {_id: 'fixture-agent', owner, sandbox_id: 'fixture-sandbox', workspace_id: 'fixture-workspace',
      worktree_id: 'fixture-worktree', status: 'starting',
      record: {origin: {operation_id: operationId}, work: {step: 'wait_for_workspace', attempts: 2}}},
    sandboxes: {_id: 'fixture-sandbox', owner, status: 'active', record: {work: {step: {kind: 'none'}, attempts: 0}}},
    workspaces: {_id: 'fixture-workspace', owner, sandbox_id: 'fixture-sandbox', status: 'ready',
      record: {resolved_commit: 'a'.repeat(64), work: {step: 'none', attempts: 0}}},
    worktrees: {_id: 'fixture-worktree', owner, workspace_id: 'fixture-workspace', sandbox_id: 'fixture-sandbox',
      status: 'creating', record: {base_commit: ''}},
  };
  const snapshot = Object.fromEntries(Object.entries(records).map(([collection, row]) => [collection, {
    findOne: async query => Object.entries(query).every(([path, expected]) =>
      path.split('.').reduce((value, key) => value?.[key], row) === expected) ? row : null,
  }]));
  let closed = 0;
  const d = {getName: () => 'fixture', getMongo: () => ({startSession: async () => ({
    startTransaction: async options => assert.equal(options.readConcern.level, 'snapshot'),
    getDatabase: name => { assert.equal(name, 'fixture'); return snapshot; },
    endSession: async () => { closed += 1; },
  })})};
  let result;
  const read = async () => runInNewContext(source, {d, operationId, print: text => { result = JSON.parse(text); }});
  await read();
  assert.deepEqual(result, {
    agent: {status: 'starting', step: 'wait_for_workspace', attempts: 2},
    sandbox: {status: 'active', step: 'none', attempts: 0},
    workspace: {status: 'ready', step: 'none', attempts: 0, commit_resolved: true},
    worktree: {status: 'creating', base_bound: false},
  });
  records.workspaces.owner = 'foreign-fixture-owner';
  await read();
  assert.equal(result.workspace, null);
  assert.equal(result.worktree, null);
  records.agents.record.origin.operation_id = 'another-operation';
  await read();
  assert.equal(result.agent, null);
  assert.equal(closed, 3);
});

test('post-acceptance attribution failure retains command and stream receipts without replay', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'multi-am-custody-'));
  const owner = {agent_id: 'fixture-agent', am: 'a', epoch: 1, instance_id: 'instance-a', pod_uid: 'pod-a'};
  const topology = {shared_database: true, coordination_enabled: true, ams: {
    a: {ready: true, instance_id: 'instance-a', pod_uid: 'pod-a'},
    b: {ready: true, instance_id: 'instance-b', pod_uid: 'pod-b'}}};
  try {
    for (const stream of [false, true]) for (const moved of [false, true]) {
      let reads = 0, calls = 0;
      const router = new NonOwnerRouter({record: async () => {}, adapter: {
        topology: async () => topology,
        ownerOf: async () => ++reads > 1 && moved ? {...owner, epoch: 2} : owner}});
      const timing = {status: 202, request_id: moved ? 'private-request' : null};
      const response = stream ? {timing, responseId: 'private-response', events: [{type: 'response.created'}],
        abort() { throw Error('must_not_abort_or_replay_here'); }, done: Promise.resolve()}
        : {status: 202, timing, body: {receipt: {command_id: 'private-command'}}};
      let failure;
      await assert.rejects(router.dispatch(owner.agent_id, async () => { calls++; return response; }), error => {
        failure = error; return error.message === (moved ? 'owner_changed_during_feature_request' : 'response_request_id_required');
      });
      assert.equal(calls, 1);
      const receipt = await retainFailure(directory, 'primary', failure);
      assert.equal(receipt.status, 202);
      assert.ok(!JSON.stringify(receipt).includes('private-'));
      const records = await Promise.all((await readdir(directory)).map(file => readFile(join(directory, file))));
      const saved = JSON.parse(records.find(raw => createHash('sha256').update(raw).digest('hex') === receipt.evidence_sha256));
      if (stream) assert.equal(saved.response.response_id, 'private-response');
      else assert.equal(saved.response.body.receipt.command_id, 'private-command');
      assert.equal(saved.response.status, 202);
      assert.deepEqual(saved.response.timing, timing);
      assert.equal('done' in saved.response, false);
      assert.equal('abort' in saved.response, false);
    }
  } finally { await rm(directory, {recursive: true, force: true}); }
});

test('primary typed conflict and restoration failure remain separate private evidence', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'multi-am-failure-'));
  const primary = Object.assign(Error('api_status_409'), {response: {status: 409,
    body: {code: 'agent_operation_in_flight', detail: 'private fixture detail'},
    timing: {request_id: 'private-fixture-request'}}});
  const restoration = Object.assign(Error('api_status_503'), {response: {status: 503,
    body: {code: 'resource_access_unavailable'}}});
  const receipts = [];
  try {
    await assert.rejects(withRestoration(async () => { throw primary; }, async () => { throw restoration; },
      async (stage, error) => { receipts.push(await retainFailure(directory, stage, error, {expected_revision: 7})); }),
    error => error === primary);
    assert.deepEqual(receipts.map(row => row.stage), ['primary', 'restoration']);
    assert.deepEqual(receipts.map(row => row.problem_code), ['agent_operation_in_flight', 'resource_access_unavailable']);
    assert.ok(!JSON.stringify(receipts).includes('private-fixture-request'));
    assert.ok(!JSON.stringify(receipts).includes('private fixture detail'));
    const records = [];
    for (const file of await readdir(directory)) {
      const path = join(directory, file), raw = await readFile(path);
      assert.equal((await stat(path)).mode & 0o077, 0);
      assert.ok(receipts.some(row => row.evidence_sha256 === createHash('sha256').update(raw).digest('hex')));
      records.push(JSON.parse(raw));
    }
    assert.equal(records.find(row => row.stage === 'primary').response.timing.request_id, 'private-fixture-request');
    assert.equal(records.find(row => row.stage === 'restoration').response.status, 503);
  } finally { await rm(directory, {recursive: true, force: true}); }
});

test('evidence-write failure cannot prevent restoration or replace the primary failure', async () => {
  const primary = Error('original_conflict');
  let restored = false;
  await assert.rejects(withRestoration(async () => { throw primary; }, async () => { restored = true; },
    async () => { throw Error('evidence_io_failure'); }), error => error === primary);
  assert.equal(restored, true);
});
