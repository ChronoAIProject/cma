import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp, readdir, readFile, rm, stat} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {createHash} from 'node:crypto';
import {journeyFailureSummary, retainFailure, withRestoration} from '../failure.mjs';
import {NonOwnerRouter} from '../routing.mjs';

test('hosted summaries keep the initiating failure and count cleanup failures without resource identities', () => {
  const result = journeyFailureSummary({
    steps: [{name: 'initialization', verdict: 'PASS'},
      {name: 'prepared_create_on_a', verdict: 'FAIL', error: 'prepared_parent_schemas_unbound'}],
    cleanup: [{public_cleanup: 'FAILED', reason: 'private fixture detail', agent_sha256: 'private-agent'},
      {am_restore: 'FAILED'}, {public_cleanup: 'PASS'}],
  });
  assert.deepEqual(result, {first_failed_step: 'prepared_create_on_a',
    first_failed_step_code: 'prepared_parent_schemas_unbound',
    journey_cleanup_failures: {public_retirement: 1, provider_release: 0, manager_restore: 1}});
  assert.equal(journeyFailureSummary({steps: [{name: 'initialization', verdict: 'FAIL', error: '/private/file'}]})
    .first_failed_step_code, 'transport_or_contract_error');
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
