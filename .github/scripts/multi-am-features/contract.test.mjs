import test from 'node:test';
import assert from 'node:assert/strict';
import {NonOwnerRouter} from './routing.mjs';
import {coverage, loadInventory} from './coverage.mjs';

const owner = {agent_id: 'agent', am: 'a', epoch: 3, instance_id: 'instance-a', pod_uid: 'pod-a'};
const topology = {shared_database: true, coordination_enabled: true, ams: {
  a: {ready: true, instance_id: 'instance-a', pod_uid: 'pod-a'},
  b: {ready: true, instance_id: 'instance-b', pod_uid: 'pod-b'},
}};
const setup = (changes = {}) => {
  const proofs = [];
  const adapter = {ownerOf: async () => owner, topology: async () => topology, ...changes};
  return {proofs, router: new NonOwnerRouter({adapter, record: p => proofs.push(p)})};
};
test('uses the live non-owner and records independent process identities', async () => {
  const {router, proofs} = setup();
  assert.equal(await router.dispatch('agent', async am => { assert.equal(am, 'b'); return 202; }), 202);
  assert.equal(proofs[0].verdict, 'PASS');
  assert.notEqual(proofs[0].owner_instance_sha256, proofs[0].ingress_instance_sha256);
  assert.ok(!JSON.stringify(proofs).includes('pod-a'));
});
test('refuses absent leases and a mismatched topology before executing', async () => {
  let calls = 0;
  for (const changes of [{ownerOf: async () => null}, {topology: async () => ({...topology, shared_database: false})}]) {
    await assert.rejects(setup(changes).router.dispatch('agent', async () => calls++));
  }
  assert.equal(calls, 0);
});
test('owner movement during a write fails attribution without replay', async () => {
  let reads = 0, calls = 0;
  const {router, proofs} = setup({ownerOf: async () => ++reads === 1 ? owner : {...owner, am: 'b', epoch: 4}});
  await assert.rejects(router.dispatch('agent', async () => ++calls), /owner_changed/);
  assert.equal(calls, 1); assert.equal(proofs[0].verdict, 'FAIL');
});
test('uncertain mutation is recorded once and never retried', async () => {
  let calls = 0; const {router, proofs} = setup();
  await assert.rejects(router.dispatch('agent', async () => { calls++; throw Error('connection lost'); }));
  assert.equal(calls, 1); assert.equal(proofs[0].verdict, 'FAIL');
});
test('all inventory identities are retained', async () => {
  const inventory = await loadInventory(new URL('./inventory/', import.meta.url).pathname);
  assert.equal(inventory.filter(x => x.role === 'user').length, 120);
  assert.equal(inventory.filter(x => x.role === 'admin').length, 76);
});
const features = [{id: 'user.send', role: 'user'}, {id: 'admin.settings', role: 'admin'}];
test('missing mapping and not-run cases cannot reduce the denominator', () => {
  const result = coverage(features, {'user.send': {cases: ['send']}}, []);
  assert.equal(result.total, 2); assert.equal(result.verdict, 'FAIL');
  assert.deepEqual(result.counts, {PASS: 0, FAIL: 0, BLOCKED: 1, NOT_RUN: 1});
});
test('passing HTTP is insufficient without non-owner evidence and all cases', () => {
  const mappings = {'user.send': {cases: ['send'], routing: 'non_owner'},
    'admin.settings': {cases: ['cas', 'stale'], routing: 'both'}};
  const rows = [{case: 'send', verdict: 'PASS'}, {case: 'cas', verdict: 'PASS', ams: ['a', 'b']}];
  assert.equal(coverage(features, mappings, rows).verdict, 'FAIL');
  rows[0].non_owner = true; rows.push({case: 'stale', verdict: 'PASS', ams: ['a', 'b']});
  assert.equal(coverage(features, mappings, rows).verdict, 'PASS');
});
test('failed repetition and missing browser observation stay failing', () => {
  const mappings = {'user.send': {cases: ['send'], minimum_runs: 3, browser: true}};
  const rows = Array.from({length: 3}, () => ({case: 'send', verdict: 'PASS'}));
  assert.equal(coverage(features.slice(0, 1), mappings, rows).verdict, 'FAIL');
  rows.forEach(row => row.real_browser = true); rows[1].verdict = 'FAIL';
  assert.equal(coverage(features.slice(0, 1), mappings, rows).verdict, 'FAIL');
});
