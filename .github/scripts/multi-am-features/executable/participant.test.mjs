import test from 'node:test';
import assert from 'node:assert/strict';
import {parentTransport, createParticipant, validateTopology, verifyRestart} from './participant.mjs';

const image = 'local/api@sha256:' + '1'.repeat(64), source = '2'.repeat(40);
const topology = () => ({shared_database: true, coordination_enabled: true, ams: {
  a: {ready: true, image, pod_uid: 'a1', instance_id: 'ia1'},
  b: {ready: true, image, pod_uid: 'b1', instance_id: 'ib1'},
}});
const owner = (am, epoch, pod_uid, instance_id) => ({agent_id: 'witness', am, epoch, pod_uid, instance_id});

test('parent actor binding never substitutes admin for an unbound or narrow actor', async () => {
  const calls = [], transport = parentTransport({bearer: 'private-user', admin_bearer: 'private-admin',
    actors: {settings_operator: {bearer: 'private-settings'}}},
  {request: async (...args) => calls.push(args)});
  await transport.request({actor: 'settings_operator', am: 'b', method: 'GET', path: '/api/v1/settings'});
  assert.equal(calls[0][0], 'settings_operator');
  await assert.rejects(transport.request({actor: 'outsider'}), /actor_unavailable_outsider/);
  assert.equal(calls.length, 1);
});

test('image, distinct process, actual exit, higher epoch and replacement are required', () => {
  const before = {topology: topology(), owner: owner('a', 1, 'a1', 'ia1')};
  const after = {topology: topology(), owner: owner('b', 2, 'b1', 'ib1')};
  after.topology.ams.a = {...after.topology.ams.a, pod_uid: 'a2', instance_id: 'ia2'};
  const boundary = {killed_am: 'a', owner_container_exit_observed: true};
  assert.equal(verifyRestart(before, after, boundary).verified, true);
  for (const changed of [{...boundary, owner_container_exit_observed: false}, {}]) {
    assert.equal(verifyRestart(before, after, changed), null);
  }
  assert.equal(verifyRestart(before, {...after, owner: {...after.owner, epoch: 1}}, boundary), null);
  assert.equal(verifyRestart(before, {...after, topology: before.topology}, boundary), null);
  const wrong = topology(); wrong.ams.b.image = 'wrong';
  assert.throws(() => validateTopology(wrong, image), /two_ready_exact_image/);
  const duplicate = topology(); duplicate.ams.b = duplicate.ams.a;
  assert.throws(() => validateTopology(duplicate, image), /distinct_processes/);
});

test('participant runs the registry in parent phases and refuses phase replay', async () => {
  const row = {case_id: 'C275:contract', family: 'user', required_steps: ['expected_result']};
  const def = {id: row.case_id, scope: 'global', steps: row.required_steps, fixtures: [],
    async run(ctx) {await ctx.step('expected_result', async () => {
      const response = await ctx.request({method: 'GET', path: '/api/v1/example'});
      ctx.assert(response.body.ok, 'response_body');
    });}};
  const registry = {catalog: {rows: [row]}, rows: new Map([[row.case_id, row]]), cases: new Map([[row.case_id, def]])};
  const participant = await createParticipant({scope: 'LOCAL_DISPOSABLE_TWO_AM', database_disposable: true,
    single_matrix_writer: true, restart_authority: 'PARENT_ONLY', bearer: 'private-user',
    expected_api_source: source, expected_api_image: image}, {registry});
  let current = topology(), observedOwner = owner('a', 1, 'a1', 'ia1');
  const calls = [], context = {phase: 'before_restart', record: {agentId: 'witness'},
    adapter: {topology: async () => current, ownerOf: async () => observedOwner},
    services: {record: async () => {}, async request(_role, am) {
      calls.push(am); return {status: 200, body: {ok: true}, timing: {am, request_id: 'actual-' + calls.length}};
    }}};
  await participant.runPhase(context);
  assert.deepEqual(calls, ['a', 'b', 'a']);
  await assert.rejects(participant.runPhase(context), /phase_order/);
  current = topology(); current.ams.a = {...current.ams.a, pod_uid: 'a2', instance_id: 'ia2'};
  observedOwner = owner('b', 2, 'b1', 'ib1');
  await participant.runPhase({...context, phase: 'after_restart',
    restart: {killed_am: 'a', owner_container_exit_observed: true}});
  const result = await participant.finalize();
  assert.equal(result.counts.roles.user.passing, 1);
  assert.equal(result.application_processes_started, 0);
  await assert.rejects(participant.runPhase(context), /phase_order/);
});
