import test from 'node:test';
import assert from 'node:assert/strict';
import {executeCase} from './runner.mjs';
import {coverageCounts} from './counts.mjs';
import {loadRegistry, validateDefinitions} from './registry.mjs';

const image = 'localhost/api@sha256:' + '1'.repeat(64), source = '2'.repeat(40);
const row = {case_id: 'C275:contract', family: 'user', required_steps: ['step_1', 'expected_result'],
  phase_minimum_runs: {before_restart: 3, after_restart: 1}, target_seconds: null};
const environment = () => ({phase: 'before_restart', iteration: 1, am: 'a', source, image, campaignId: 'contract-campaign',
  fixtures: {}, record: async () => {}, transport: {request: async args => ({status: 200,
    body: {value: 7}, timing: {am: args.am, request_id: 'observed-request', body_ms: 1}})}});
const definition = run => ({id: row.case_id, scope: 'global', steps: row.required_steps,
  fixtures: [], run});
async function realCase(ctx) {
  let response;
  await ctx.step('step_1', async () => {
    response = await ctx.request({method: 'GET', path: '/api/v1/example'});
    ctx.assert(response.status === 200, 'read_status');
  });
  await ctx.step('expected_result', async () => ctx.assert(response.body.value === 7, 'observed_value'));
}

test('actual requests, every step, assertions and cleanup are required', async () => {
  const env = environment(), result = await executeCase(definition(realCase), row, env);
  assert.equal(result.status, 'PASS'); assert.equal(result.steps.length, 2);
  for (const run of [async () => {}, async ctx => ctx.step('step_1', async () => {})]) {
    assert.equal((await executeCase(definition(run), row, env)).status, 'FAIL');
  }
  const missing = {...definition(realCase), fixtures: ['browser']};
  const blocked = await executeCase(missing, row, env);
  assert.equal(blocked.status, 'BLOCKED'); assert.equal(blocked.requests.length, 0);
});

test('accepted response remains in custody when owner attribution fails; no replay', async () => {
  const env = environment(), events = []; let requests = 0;
  const owner = {agent_id: 'agent', am: 'a', epoch: 1, instance_id: 'owner', pod_uid: 'pod-a'};
  env.adapter = {ownerOf: async () => owner, topology: async () => ({shared_database: true,
    coordination_enabled: true, ams: {a: {ready: true, instance_id: 'owner', pod_uid: 'pod-a'},
      b: {ready: true, instance_id: 'peer', pod_uid: 'pod-b'}}})};
  env.record = async event => events.push(event);
  env.transport.request = async args => {requests += 1; assert.equal(args.am, 'b');
    return {status: 202, body: {receipt: {command_id: 'original-command'}}, timing: {am: args.am, request_id: ''}};};
  const def = {...definition(async ctx => ctx.step('step_1', async () => {
    await ctx.request({method: 'POST', path: '/api/v2/agents/agent/commands',
      agentId: 'agent', key: 'original-key', allowed: [202]});
  })), scope: 'agent'};
  const result = await executeCase(def, row, env);
  assert.equal(result.status, 'FAIL'); assert.equal(requests, 1);
  assert.equal(events.find(e => e.kind === 'primary_failure').response.body.receipt.command_id, 'original-command');
  assert.equal(result.owner_proofs[0].verdict, 'FAIL');
});

test('cleanup preserves primary failure and continues through all owned resources', async () => {
  const called = [], events = [], env = environment(); env.record = async e => events.push(e);
  const result = await executeCase(definition(async ctx => {
    ctx.defer('first_resource', async () => called.push('first'));
    ctx.defer('second_resource', async () => {called.push('second'); throw Error('cleanup_refused');});
    await ctx.step('step_1', async () => ctx.assert(false, 'original_failure'));
  }), row, env);
  assert.equal(result.status, 'FAIL'); assert.equal(result.primary_failure, 'original_failure');
  assert.deepEqual(called, ['second', 'first']);
  assert.deepEqual(result.cleanup.map(x => x.status), ['FAIL', 'PASS']);
  assert.equal(events.filter(e => e.kind === 'primary_failure').length, 1);
});

test('counts refuse stale-image, missing steps, duplicate repetitions and failed later attempts', async () => {
  const def = definition(realCase), registry = {catalog: {rows: [row]}, cases: new Map([[row.case_id, def]])};
  const env = environment(); const baseline = await executeCase(def, row, env);
  const results = [1, 2, 3].map(iteration => ({...baseline, iteration,
    requests: baseline.requests.map(request => ({...request, am: iteration === 2 ? 'b' : 'a'}))}));
  results.push({...baseline, phase: 'after_restart'});
  const bind = {source, image, campaign_id: 'contract-campaign', fixtures: [], owner_restart: {verified: true, evidence_sha256: '4'.repeat(64)}};
  assert.equal(coverageCounts(registry, results, bind).roles.user.passing, 1);
  assert.equal(coverageCounts(registry, [], bind).roles.user.executable, 0);
  assert.equal(coverageCounts(registry, [], {...bind, executable_case_ids: [row.case_id]}).roles.user.executable, 1);
  assert.equal(coverageCounts(registry, results, {...bind, campaign_id: 'other-run'}).roles.user.passing, 0);
  assert.equal(coverageCounts(registry, results.map(result => ({...result, requires_parent_cleanup: true})), bind).roles.user.passing, 0);
  assert.equal(coverageCounts(registry, results, {...bind, owner_restart: null}).roles.user.passing, 0);
  assert.equal(coverageCounts(registry, results.map(result => ({...result,
    requests: result.requests.map(request => ({...request, am: 'a'}))})), bind).roles.user.passing, 0);
  assert.equal(coverageCounts(registry, results, {...bind, source: '3'.repeat(40)}).roles.user.passing, 0);
  assert.equal(coverageCounts(registry, results.map(r => ({...r, iteration: 1})), bind).roles.user.passing, 0);
  assert.equal(coverageCounts(registry, results.map(r => ({...r, steps: []})), bind).roles.user.passing, 0);
  assert.equal(coverageCounts(registry, [...results, {...baseline, status: 'FAIL'}], bind).roles.user.failing, 1);
});

test('evidence sink failure cannot skip cleanup or leave a passing result', async () => {
  const env = environment(), cleaned = [];
  env.record = async () => {throw Error('private_sink_failure');};
  const result = await executeCase(definition(async ctx => {
    ctx.defer('owned_resource', async () => cleaned.push(true));
    await realCase(ctx);
  }), row, env);
  assert.equal(result.status, 'FAIL'); assert.equal(result.evidence_error, 'private_evidence_write_failed');
  assert.deepEqual(cleaned, [true]);
});

test('explicit am reaches owner and non-owner exactly; omitted am selects non-owner', async () => {
  const env = environment(), seen = [];
  const owner = {agent_id: 'owned', am: 'a', epoch: 9, instance_id: 'ia', pod_uid: 'pa'};
  env.adapter = {ownerOf: async () => owner, topology: async () => ({shared_database: true,
    coordination_enabled: true, ams: {a: {ready: true, instance_id: 'ia', pod_uid: 'pa'},
      b: {ready: true, instance_id: 'ib', pod_uid: 'pb'}}})};
  env.transport.request = async request => {
    seen.push(request.am);
    return {status: 200, body: {served: request.am},
      timing: {am: request.am, request_id: 'actual-' + seen.length}};
  };
  const result = await executeCase({...definition(async ctx => {
    await ctx.step('step_1', async () => {
      const write = await ctx.request({agentId: 'owned', am: 'a', method: 'POST', path: '/api/v1/agents/owned'});
      const read = await ctx.request({agentId: 'owned', am: 'b', method: 'GET', path: '/api/v1/agents/owned'});
      ctx.assert(write.body.served === 'a' && read.body.served === 'b', 'actual_cross_am');
    });
    await ctx.step('expected_result', async () => {
      const selected = await ctx.request({agentId: 'owned', method: 'GET', path: '/api/v1/agents/owned'});
      ctx.assert(selected.body.served === 'b', 'default_non_owner');
    });
  }), scope: 'agent'}, row, env);
  assert.equal(result.status, 'PASS'); assert.deepEqual(seen, ['a', 'b', 'b']);
  assert.deepEqual(result.owner_proofs.map(proof => [proof.owner, proof.ingress, proof.kind]),
    [['a', 'a', 'explicit_ingress'], ['a', 'b', 'explicit_ingress'], ['a', 'b', 'non_owner']]);
  assert.ok(result.owner_proofs.every(proof => proof.owner_epoch === 9 && proof.verdict === 'PASS'));
});

test('frozen registry preserves every denominator and refuses cross-author cases', async () => {
  const registry = await loadRegistry();
  assert.equal(registry.rows.size, 362); assert.equal(registry.owners.size, 362);
  const assignments = [...new Set(registry.owners.values())].map(owner => ({owner,
    case_ids: [...registry.owners].filter(([, v]) => v === owner).map(([id]) => id)}));
  const id = [...registry.owners].find(([, owner]) => owner === 'developer-06')[0];
  const actual = registry.rows.get(id);
  assert.throws(() => validateDefinitions(registry.catalog, assignments,
    [{owner: 'developer-17', definitions: [{id, scope: 'global', fixtures: [],
      steps: actual.required_steps, run: async () => {}}]}]), /case_owner_or_duplicate/);
});
