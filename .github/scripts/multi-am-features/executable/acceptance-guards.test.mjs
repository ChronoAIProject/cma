import test from 'node:test';
import assert from 'node:assert/strict';
import {executeCase} from './runner.mjs';
import {coverageCounts} from './counts.mjs';
import {latencyBoundary} from './latency.mjs';
import {validEgReceipt, validPlatformProof} from './evidence-scope.mjs';
import {loadRegistry} from './registry.mjs';

const source = '1'.repeat(40), image = 'fixture@sha256:' + '2'.repeat(64);
const binding = {source, image, campaign_id: 'guards', fixtures: [],
  eg_source: source, eg_image: image,
  owner_restart: {verified: true, evidence_sha256: '3'.repeat(64)}};
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
function receipt(am = 'a', eg = false) {
  const serving = {schema: eg ? 'c275-eg-serving-v1' : 'c275-serving-v1',
    ...(eg ? {scope: 'eg', replica: am, source, image} : {am}), ready: true,
    origin: 'http://127.0.0.1:' + (am === 'a' ? 1111 : 2222), pod_uid: 'pod-' + am,
    instance_id: 'instance-' + am, container_id: 'container-' + am, observed_at: '2026-11-16T10:00:00.000Z'};
  return {...(eg ? {scope: 'eg', replica: am} : {am}), request_id: 'request-' + am,
    serving, started_at: '2026-11-16T10:00:00.001Z', ended_at: '2026-11-16T10:00:00.002Z',
    serving_after: {...serving, observed_at: '2026-11-16T10:00:00.003Z'},
    attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}};
}
function registry(row, definition) {return {catalog: {rows: [row]}, cases: new Map([[row.case_id, definition]])};}
function repetitions(result, eg = false) {
  return [1, 2, 3].map(iteration => ({...result, iteration,
    requests: eg ? [] : [receipt(iteration === 2 ? 'b' : 'a')],
    eg_requests: eg ? [receipt(iteration === 2 ? 'b' : 'a', true)] : []}));
}
const baseRow = {case_id: 'C275:cma-user.task.create', family: 'user', required_steps: ['expected_result'],
  phase_minimum_runs: {steady: 3}, target_seconds: .01};
async function actionResult({setup = 0, action = 0, cleanup = 0, measure = true, fail = false} = {}) {
  const definition = {id: baseRow.case_id, fixtures: [], scope: 'global', steps: baseRow.required_steps,
    async run(ctx) {
      await delay(setup); ctx.defer('cleanup', () => delay(cleanup));
      await ctx.step('expected_result', async () => {
        const work = async () => {await delay(action); if (fail) throw Error('action_refused'); ctx.assert(true, 'usable');};
        if (measure) await ctx.measureAction('create_to_usable', work); else await work();
        await ctx.request({method: 'GET', path: '/api/v1/example'});
      });
    }};
  const result = await executeCase(definition, baseRow, {phase: 'steady', iteration: 1, am: 'a', source, image,
    campaignId: 'guards', transport: {request: async () => ({status: 200, body: {}, timing: receipt()})}});
  return {result, definition};
}

test('action target excludes setup/cleanup, keeps failures and refuses missing timing', async () => {
  const fast = await actionResult({setup: 25, cleanup: 25});
  assert.equal(fast.result.status, 'PASS'); assert.equal(fast.result.performance, 'PASS');
  assert.ok(fast.result.elapsed_ms > 40);
  const slow = await actionResult({action: 25});
  assert.equal(slow.result.functional_status, 'PASS'); assert.equal(slow.result.status, 'FAIL');
  const failed = await actionResult({fail: true});
  assert.equal(failed.result.action_timings[0].status, 'FAIL');
  assert.equal(failed.result.primary_failure, 'action_refused');
  assert.equal((await actionResult({measure: false})).result.status, 'BLOCKED');
  const r = registry(baseRow, fast.definition);
  assert.equal(coverageCounts(r, repetitions(fast.result), binding).roles.user.passing, 1);
  for (const sample of [{...fast.result, performance: 'FAIL'}, {...fast.result, action_timings: []},
    {...fast.result, action_timings: [{boundary: 'create_to_usable', status: 'PASS', elapsed_ms: 60000}]}])
    assert.equal(coverageCounts(r, repetitions(sample), binding).roles.user.passing, 0);
  const catalog = await loadRegistry();
  const targeted = catalog.catalog.rows.filter(row => row.target_seconds != null);
  assert.equal(targeted.length, 16); assert.ok(targeted.every(latencyBoundary));
});

test('EG-only rows require genuine EG process windows and an independent AM restart witness', async () => {
  const row = {...baseRow, case_id: 'C275:EG-077', family: 'eg', target_seconds: null};
  const definition = {id: row.case_id, fixtures: [], scope: 'eg', evidence_scope: 'eg_only', steps: row.required_steps,
    run: ctx => ctx.step('expected_result', async () => {
      await ctx.observe('eg_request', async () => ({schema: 'multi-eg-request-v1', receipt: receipt('a', true)}));
      ctx.assert(true, 'eg_effect');
    })};
  const result = await executeCase(definition, row, {phase: 'steady', iteration: 1, am: 'a', source, image, campaignId: 'guards'});
  assert.equal(result.status, 'PASS'); assert.deepEqual(result.requests, []);
  const r = registry(row, definition), results = repetitions(result, true);
  assert.equal(coverageCounts(r, results, binding).roles.eg.passing, 1);
  assert.equal(coverageCounts(r, results, {...binding, owner_restart: null}).roles.eg.passing, 0);
  assert.equal(coverageCounts(r, results, {...binding, eg_image: 'wrong'}).roles.eg.passing, 0);
  assert.equal(coverageCounts(r, results, {...binding, eg_source: undefined}).roles.eg.passing, 0);
  assert.equal(coverageCounts(r, repetitions({...result, eg_requests: []}), binding).roles.eg.passing, 0);
  for (const change of [value => {delete value.serving_after;}, value => {value.serving_after.instance_id = 'changed';},
    value => {value.serving.draining = true;}, value => {value.am = 'a';}, value => {value.serving_after.source = 'f'.repeat(40);}]) {
    const invalid = structuredClone(receipt('a', true)); change(invalid); assert.equal(validEgReceipt(invalid), false);
    assert.equal(coverageCounts(r, results.map(item => ({...item, eg_requests: [invalid]})), binding).roles.eg.passing, 0);
  }
});

test('local relay success cannot satisfy the three actual external platform rows', () => {
  for (const [number, platform] of [[32, 'telegram'], [33, 'discord'], [34, 'whatsapp']]) {
    const row = {...baseRow, case_id: 'C275:EG-0' + number, family: 'eg', target_seconds: null};
    const definition = {id: row.case_id, fixtures: [], scope: 'eg'};
    const event = {event_sha256: 'a'.repeat(64), conversation_sha256: 'b'.repeat(64), response_sha256: 'c'.repeat(64)};
    const proof = {schema: 'multi-am-external-platform-v1', kind: 'actual_platform', platform, synthetic: false, ...event,
      delivery: {platform, observed: true, artifact_sha256: 'd'.repeat(64)},
      sender: {platform, authenticated: true, independent: true, artifact_sha256: 'e'.repeat(64)}};
    const result = {id: row.case_id, family: 'eg', source, image, campaign_id: 'guards', phase: 'steady', status: 'PASS',
      platform_events: [event], external_platform_proofs: [],
      steps: [{id: 'expected_result', status: 'PASS', assertions: 1}], cleanup: [],
      owner_proofs: [{owner: 'a', ingress: 'b', verdict: 'PASS', owner_epoch: 1}]};
    const r = registry(row, definition);
    assert.equal(coverageCounts(r, repetitions(result), binding).roles.eg.passing, 0);
    assert.equal(coverageCounts(r, repetitions(result), binding).roles.eg.blocked, 1);
    assert.equal(validPlatformProof({...proof, synthetic: true}, platform, event), false);
    assert.equal(validPlatformProof({...proof, sender: {...proof.sender, independent: false}}, platform, event), false);
    assert.equal(validPlatformProof({...proof, response_sha256: 'f'.repeat(64)}, platform, event), false);
    assert.equal(coverageCounts(r, repetitions({...result, external_platform_proofs: [proof]}), binding).roles.eg.passing, 1);
  }
});
