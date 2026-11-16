import test from 'node:test';
import assert from 'node:assert/strict';
import {parentTransport, createParticipant, validateTopology, verifyRestart, provisionalCounts} from './participant.mjs';

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
    run_id: 'contract-campaign',
    single_matrix_writer: true, restart_authority: 'PARENT_ONLY', bearer: 'private-user',
    expected_api_source: source, expected_api_image: image}, {registry});
  let current = topology(), observedOwner = owner('a', 1, 'a1', 'ia1');
  const calls = [], recorded = [], context = {phase: 'before_restart', record: {agentId: 'witness'},
    adapter: {topology: async () => current, ownerOf: async () => observedOwner},
    services: {record: async event => recorded.push(structuredClone(event)), async request(_role, am) {
      const serving = {schema: 'c275-serving-v1', am, ready: true, ...current.ams[am],
        origin: 'http://127.0.0.1:' + (am === 'a' ? 10001 : 10002), container_id: 'container-' + current.ams[am].pod_uid,
        observed_at: '2026-11-16T10:00:00.000Z'};
      calls.push(am); return {status: 200, body: {ok: true}, timing: {am, request_id: 'actual-' + calls.length,
        serving, started_at: '2026-11-16T10:00:00.001Z', ended_at: '2026-11-16T10:00:00.002Z',
        serving_after: {...serving, observed_at: '2026-11-16T10:00:00.003Z'},
        attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}}};
    }}};
  await participant.runPhase(context);
  assert.deepEqual(calls, ['a', 'b', 'a']);
  const progress = recorded.filter(event => event.kind === 'executable_progress');
  assert.deepEqual(progress.map(event => event.trigger), ['phase_start', 'fixtures_bound', 'case_outcome', 'case_outcome', 'case_outcome']);
  assert.deepEqual(progress.slice(2).map(event => event.provisional_roles.user.attempts), [1, 2, 3]);
  assert.ok(progress.every(event => event.accepted_roles.user.passing === 0 && !event.runtime_acceptance));
  assert.equal(progress.at(-1).provisional_roles.user.passing, 1);
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

test('provisional failures remain sticky and never substitute for restart acceptance', () => {
  const row = {case_id: 'C275:progress', family: 'user', required_steps: ['expected_result']};
  const registry = {catalog: {rows: [row]}, cases: new Map([[row.case_id, {fixtures: [], scope: 'global'}]])};
  const common = {id: row.case_id, family: 'user', phase: 'before_restart', source, image, campaign_id: 'progress'};
  const value = provisionalCounts(registry, [{...common, status: 'FAIL'}, {...common, status: 'PASS'}],
    {source, image, campaign_id: 'progress'}, 'before_restart');
  assert.equal(value.provisional_roles.user.failing, 1);
  assert.equal(value.provisional_roles.user.passing, 0);
  assert.equal(value.accepted_roles.user.passing, 0);
});

test('finalization cannot label an unexecuted registry PASS', async () => {
  const row = {case_id: 'C275:missing-browser', family: 'user', required_steps: ['expected_result']};
  const definition = {id: row.case_id, scope: 'browser', fixtures: ['browser'], steps: row.required_steps,
    run: async () => {throw Error('must_not_execute');}};
  const registry = {catalog: {rows: [row]}, rows: new Map([[row.case_id, row]]),
    cases: new Map([[row.case_id, definition]])};
  const participant = await createParticipant({scope: 'LOCAL_DISPOSABLE_TWO_AM', database_disposable: true,
    run_id: 'missing-browser', single_matrix_writer: true, restart_authority: 'PARENT_ONLY',
    expected_api_source: source, expected_api_image: image}, {registry});
  const result = await participant.finalize();
  assert.equal(result.verdict, 'INCOMPLETE');
  assert.equal(result.runtime_acceptance, false);
  assert.equal(result.counts.roles.user.blocked, 1);
  assert.equal(result.counts.roles.user.passing, 0);
});

test('phase budget stops enumeration before spending the reserved cleanup interval', async () => {
  const rows = ['first', 'second'].map(id => ({case_id: 'C275:' + id, family: 'user', required_steps: ['expected_result']}));
  const started = [], events = [];
  const registry = {catalog: {rows}, rows: new Map(rows.map(row => [row.case_id, row])),
    cases: new Map(rows.map(row => [row.case_id, {id: row.case_id, scope: 'global', fixtures: [], steps: row.required_steps,
      run: ctx => ctx.step('expected_result', async () => {
        started.push(row.case_id); await new Promise(resolve => setTimeout(resolve, 30)); ctx.assert(true, 'finished_work');
      })}]))};
  const participant = await createParticipant({scope: 'LOCAL_DISPOSABLE_TWO_AM', database_disposable: true,
    run_id: 'phase-budget', single_matrix_writer: true, restart_authority: 'PARENT_ONLY',
    expected_api_source: source, expected_api_image: image, executable_case_budget_ms: 5,
    executable_phase_budget_ms: 35, executable_cleanup_budget_ms: 20}, {registry});
  await participant.runPhase({phase: 'before_restart', record: {agentId: 'witness'},
    adapter: {topology: async () => topology(), ownerOf: async () => owner('a', 1, 'a1', 'ia1')},
    services: {record: async event => events.push(event)}});
  assert.deepEqual(started, ['C275:first']);
  assert.ok(events.some(event => event.trigger === 'phase_budget_exhausted'));
  assert.equal(events.filter(event => event.trigger === 'case_outcome').length, 1);
  assert.equal((await participant.finalize()).runtime_acceptance, false);
});
