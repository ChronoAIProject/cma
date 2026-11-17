import test from 'node:test';
import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {setImmediate as nextTurn} from 'node:timers/promises';
import {executeCase} from '../../runner.mjs';
import {releaseRace, releaseWinner} from './e1.mjs';

const source = 'a'.repeat(40), image = 'eg@sha256:' + 'b'.repeat(64);
const path = '/v1/channels/synthetic/partitions/synthetic/release';
const context = {assert: (condition, code) => assert.ok(condition, code),
  observe: async (_label, operation) => operation()};
const replicas = ['a', 'b'].map(slot => ({slot, instance: 'synthetic-eg-' + slot}));

// Protocol doubles for case lifecycle tests, not product or serving evidence.
function response(replica, status, body) {
  const stamp = ms => `2026-11-16T10:00:00.${String(ms).padStart(3, '0')}Z`;
  const requestId = 'synthetic-release-' + replica;
  const serving = {schema: 'c275-eg-serving-v1', scope: 'eg', replica, ready: true,
    origin: 'http://127.0.0.1:' + (replica === 'a' ? 12345 : 12346),
    pod_uid: 'synthetic-pod-' + replica, instance_id: 'synthetic-eg-' + replica,
    container_id: 'synthetic-container-' + replica, source, image, observed_at: stamp(0)};
  return {status, body, instance: serving.instance_id, requestId,
    eg_receipt: {scope: 'eg', replica, request_id: requestId, method: 'POST', status,
      serving, serving_after: {...serving, observed_at: stamp(10)},
      started_at: stamp(1), ended_at: stamp(9),
      attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}}};
}

function commands() {
  const expected_head = randomUUID();
  return ['a', 'b'].map(() => ({operation_id: randomUUID(), expected_head, expected_epoch: 2}));
}

for (const cleanupFails of [false, true]) test(
  `E1 joins both release POSTs before runner cleanup (cleanup ${cleanupFails ? 'fails' : 'passes'})`, async () => {
  let unblock, notifyStarted, notifyFinished, cleanups = 0, completed = false;
  const gate = new Promise(resolve => {unblock = resolve;});
  const started = new Promise(resolve => {notifyStarted = resolve;});
  const finished = new Promise(resolve => {notifyFinished = resolve;});
  const calls = [], records = [], requests = commands();
  const accepted = response('b', 200, {operation_id: requests[1].operation_id,
    released_head: requests[1].expected_head, disposition: 'unknown_abandoned', epoch: 3, released_at_ms: 1000});
  const lease = {replicas, async request(request) {
    calls.push(request);
    if (request.replica === 'a') throw Error('synthetic_release_transport_unknown');
    notifyStarted(); await gate;
    notifyFinished(); return accepted;
  }};
  const definition = {id: 'E1:EG-2171', scope: 'eg', fixtures: [], steps: ['step_4'],
    async run(ctx) {
      ctx.defer('close_release_fixture', async () => {
        cleanups++;
        if (cleanupFails) throw Error('synthetic_release_cleanup_failed');
      });
      await ctx.step('step_4', () => releaseRace(ctx, lease, path, requests));
    }};
  const execution = executeCase(definition, {id: 'EG-E1', case_id: definition.id,
    family: 'eg', required_steps: definition.steps}, {phase: 'steady', iteration: 1, am: 'a',
    source, image, record: async value => records.push(value)}).then(result => {completed = true; return result;});
  await started;
  await nextTurn();
  const earlyCleanup = cleanups, earlyCompletion = completed;
  unblock();
  const result = await execution;
  await finished;
  assert.equal(earlyCleanup, 0, 'cleanup must wait for the other mutation');
  assert.equal(earlyCompletion, false, 'the case must retain the live request');
  assert.equal(cleanups, 1);
  assert.deepEqual(calls.map(call => call.replica), ['a', 'b']);
  assert.equal(result.status, 'FAIL');
  assert.equal(result.primary_failure, 'e1_release_race_incomplete');
  assert.equal(result.cleanup[0].status, cleanupFails ? 'FAIL' : 'PASS');
  if (cleanupFails) assert.equal(result.cleanup[0].error_code, 'synthetic_release_cleanup_failed');
  assert.equal(result.eg_requests.length, 1, 'late acknowledged response still retains attribution');
  const failure = records.find(record => record.kind === 'primary_failure');
  assert.deepEqual(failure.response.releases, [
    {replica: 'a', outcome: 'rejected'},
    {replica: 'b', outcome: 'fulfilled', response: accepted},
  ], 'retain the acknowledged release receipt even when the other result is unknown');
});

test('E1 retains both release failures and never repeats an uncertain request', async () => {
  let unblock, notifyStarted;
  const gate = new Promise(resolve => {unblock = resolve;});
  const started = new Promise(resolve => {notifyStarted = resolve;});
  const failures = [Error('synthetic_first_unknown'), Error('synthetic_second_unknown')], calls = [];
  const lease = {replicas, async request(request) {
    calls.push(request.replica);
    if (request.replica === 'b') {notifyStarted(); await gate;}
    throw failures[request.replica === 'a' ? 0 : 1];
  }};
  const outcome = releaseRace(context, lease, path, commands()).then(() => null, error => error);
  await started; await nextTurn(); unblock();
  const error = await outcome;
  assert.ok(error instanceof AggregateError);
  assert.deepEqual(error.errors, failures);
  assert.deepEqual(error.response.releases, ['a', 'b'].map(replica => ({replica, outcome: 'rejected'})));
  assert.deepEqual(calls, ['a', 'b']);
});

test('E1 release race preserves replica order and the original winner receipt', async () => {
  const requests = commands(), calls = [];
  const replies = [response('a', 409, {detail: 'routed_release_conflict'}),
    response('b', 200, {operation_id: requests[1].operation_id, released_head: requests[1].expected_head,
      disposition: 'unknown_abandoned', epoch: 3, released_at_ms: 1000})];
  const lease = {replicas, async request(request) {
    calls.push(request);
    if (request.replica === 'a') await nextTurn();
    return replies[request.replica === 'a' ? 0 : 1];
  }};
  const result = await releaseRace(context, lease, path, requests);
  assert.equal(result[0], replies[0]); assert.equal(result[1], replies[1]);
  assert.equal(releaseWinner(context, result, requests).command, requests[1]);
  assert.deepEqual(calls.map(({method, actor, body}) => ({method, actor, body})),
    requests.map(body => ({method: 'POST', actor: 'release_operator', body})));
});
