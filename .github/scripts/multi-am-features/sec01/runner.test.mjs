import test from 'node:test';
import assert from 'node:assert/strict';
import {cells, CELL_COUNT, CATEGORIES, STATES, DIRECTIONS} from './cells.mjs';
import {evaluateProbe} from './evaluate.mjs';
import {probeSpecs, runSec01} from './runner.mjs';

const HASH = 'a'.repeat(64);
const SOURCE = 'b'.repeat(40);
const IMAGE = '@sha256:' + 'c'.repeat(64);
const baseBinding = (cell, run_id) => ({
  schema: 'sec01-runtime-binding-v1', run_id, cell_id: cell.id, source_sha: SOURCE, image_digest: IMAGE,
  nonce: 'nonce-' + cell.id.toLowerCase(),
  state: cell.state, direction: cell.direction, runtime_id: 'runtime-' + cell.id.toLowerCase(),
  incarnation: 'incarnation-' + cell.id.toLowerCase(),
  ams: {a: {instance_id: 'instance-a', pod_uid: 'pod-a', ready: true},
    b: {instance_id: 'instance-b', pod_uid: 'pod-b', ready: true}},
  tenant: {owner: cell.owner_tenant, foreign: cell.foreign_tenant}, evidence_sha256: HASH,
  lifecycle: {state: cell.state, evidence_sha256: HASH,
    new_runtime: cell.state === 'cold', tenant_history: cell.state !== 'cold',
    reused_after_retirement: cell.state === 'warm', sanitized: cell.state === 'warm',
    paused: cell.state === 'resumed', authorization_changed_while_paused: cell.state === 'resumed',
    restored_normally: cell.state === 'resumed'},
  serving: [{am: 'a', verdict: 'PASS', evidence_sha256: HASH}, {am: 'b', verdict: 'PASS', evidence_sha256: HASH}],
  cleanup: async () => {},
  targets: Object.fromEntries(CATEGORIES[cell.category].probes.map(probe => [probe,
    {tenant: cell.foreign_tenant, exists: true, run_id, evidence_sha256: HASH}])),
  positives: Object.fromEntries(CATEGORIES[cell.category].positive.map(name => [name, {sha256: HASH, bytes: 4}])),
});

function observation(spec, binding, context) {
  const base = {schema: 'sec01-observation-v1', id: spec.id, cell_id: spec.cell_id,
    state: spec.state, direction: spec.direction, run_id: context.run_id,
    runtime_id: binding.runtime_id, incarnation: binding.incarnation, am: 'a',
    tenant: {owner: context.owner_tenant, foreign: context.foreign_tenant}, evidence_sha256: HASH};
  if (spec.expect === 'allow') return {...base, kind: spec.kind, status: 200,
    body_sha256: HASH, functional_operation: spec.operation, private_bytes: 0, private_fields: 0};
  const policy = {decision: 'deny', evidence_sha256: HASH, target_id: spec.id,
    nonce: context.nonce, runtime_id: context.runtime_id, run_id: context.run_id};
  if (spec.kind === 'tcp') return {...base, kind: 'tcp', addresses: [{connected: false, errno: 13}], policy};
  if (spec.kind === 'file') return {...base, kind: 'file', operations: [
    {operation: 'read', succeeded: false, errno: 13}, {operation: 'enumerate', succeeded: false, errno: 13},
    {operation: 'write_open', succeeded: false, errno: 13}], policy};
  return {...base, kind: spec.kind, status: 403, code: spec.denial_code,
    private_bytes: 0, private_fields: 0, redirect_authorization_count: 0,
    redirect_cookie_count: 0, old_token_reuse_count: 0, unauthorized_mint_count: 0,
    issuer_ledger_sha256: HASH, receiver_ledger_sha256: HASH, policy,
    ...(spec.scenario === 'revoked_credential' ? {revocation_receipt_sha256: HASH,
      revocation_sequence: 1, attempt_sequence: 2} : {})};
}

function passingFixture() {
  const fixture = {
    async prepareCell({cell, run_id}) { return baseBinding(cell, run_id); },
    async runtimeProbe({spec, binding, context}) { return observation(spec, binding, context); },
    async functionalProbe({spec, binding, context}) { return observation(spec, binding, context); },
    async credentialProbe({spec, binding, context}) { return observation(spec, binding, context); },
  };
  return fixture;
}

test('expands exactly five categories x three states x two tenant directions', () => {
  assert.equal(CELL_COUNT, 30);
  assert.equal(cells.length, 30);
  assert.equal(new Set(cells.map(cell => cell.id)).size, 30);
  assert.deepEqual(Object.keys(CATEGORIES), ['network', 'file', 'metadata', 'kubernetes', 'credential']);
  assert.deepEqual(STATES, ['cold', 'warm', 'resumed']);
  assert.deepEqual(DIRECTIONS.map(x => x.id), ['a_to_b', 'b_to_a']);
  for (const category of Object.keys(CATEGORIES)) assert.equal(cells.filter(x => x.category === category).length, 6);
});

test('a fixture without runtime hooks retains all cells as BLOCKED', async () => {
  const packet = await runSec01({fixture: {}, source: SOURCE, image: IMAGE, runId: 'missing-hooks', record: async () => {}});
  assert.equal(packet.runtime_cells, 30);
  assert.equal(packet.results.length, 30);
  assert.equal(packet.verdict, 'BLOCKED');
  assert.equal(packet.results.filter(x => x.status === 'BLOCKED').length, 30);
  assert.equal(packet.results.filter(x => x.status === 'PASS').length, 0);
  for (const cell of cells) assert.deepEqual(packet.counts[cell.id], {PASS: 0, FAIL: 0, BLOCKED: 1, NOT_RUN: 0});
});

test('the complete fixture contract executes every cell and both directions', async () => {
  const packet = await runSec01({fixture: passingFixture(), source: SOURCE, image: IMAGE,
    runId: 'complete-contract', record: async () => {}});
  assert.equal(packet.verdict, 'PASS');
  assert.equal(packet.results.length, 30);
  assert.equal(packet.results.filter(x => x.status === 'PASS').length, 30);
  assert.deepEqual(Object.fromEntries(Object.entries(packet.by_category).map(([key, value]) => [key, value.PASS])),
    {network: 6, file: 6, metadata: 6, kubernetes: 6, credential: 6});
  for (const cell of cells) assert.equal(packet.counts[cell.id].PASS, 1);
});

test('a successful prohibited TCP connection is FAIL even with an application refusal', () => {
  const cell = cells.find(x => x.category === 'network');
  const spec = probeSpecs(cell, {targets: {peer_ipv4: {tenant: cell.foreign_tenant,
    exists: true, run_id: 'r', evidence_sha256: HASH}}})[0];
  const result = evaluateProbe(spec, {id: spec.id, kind: 'tcp', addresses: [{connected: true}],
    policy: {decision: 'deny', evidence_sha256: HASH, target_id: spec.id, nonce: 'n', runtime_id: 'rt', run_id: 'r'}},
    {run_id: 'r', nonce: 'n', runtime_id: 'rt', owner_tenant: cell.owner_tenant, foreign_tenant: cell.foreign_tenant});
  assert.equal(result.status, 'FAIL');
  assert.equal(result.reason, 'prohibited_tcp_connected');
});

test('missing owner-side file existence is BLOCKED rather than a denial PASS', () => {
  const cell = cells.find(x => x.category === 'file');
  const spec = probeSpecs(cell, {targets: {foreign_home: {tenant: cell.foreign_tenant,
    exists: false, run_id: 'r', evidence_sha256: HASH}}})[0];
  const observation = {id: spec.id, kind: 'file', operations: [
    {operation: 'read', succeeded: false, errno: 2}, {operation: 'enumerate', succeeded: false, errno: 2},
    {operation: 'write_open', succeeded: false, errno: 2}], policy: {decision: 'deny', evidence_sha256: HASH,
      target_id: spec.id, nonce: 'n', runtime_id: 'rt', run_id: 'r'}};
  const result = evaluateProbe(spec, observation, {run_id: 'r', nonce: 'n', runtime_id: 'rt',
    owner_tenant: cell.owner_tenant, foreign_tenant: cell.foreign_tenant});
  assert.equal(result.status, 'BLOCKED');
  assert.equal(result.reason, 'foreign_canary_existence_unproved');
});

test('credential revocation requires ordered issuer and receiver evidence', () => {
  const cell = cells.find(x => x.category === 'credential' && x.state === 'resumed');
  const spec = probeSpecs(cell, {positives: {}}).find(x => x.scenario === 'revoked_credential');
  const context = {run_id: 'r', nonce: 'n', runtime_id: 'rt', owner_tenant: cell.owner_tenant, foreign_tenant: cell.foreign_tenant};
  const incomplete = {id: spec.id, kind: 'credential', status: 403, code: spec.denial_code,
    private_bytes: 0, private_fields: 0, policy: {decision: 'deny', evidence_sha256: HASH,
      target_id: spec.id, nonce: 'n', runtime_id: 'rt', run_id: 'r'}};
  assert.equal(evaluateProbe(spec, incomplete, context).status, 'BLOCKED');
  const complete = {...incomplete, issuer_ledger_sha256: HASH, receiver_ledger_sha256: HASH,
    redirect_authorization_count: 0, redirect_cookie_count: 0, old_token_reuse_count: 0,
    unauthorized_mint_count: 0, revocation_receipt_sha256: HASH, revocation_sequence: 1, attempt_sequence: 2};
  assert.equal(evaluateProbe(spec, complete, context).status, 'PASS');
});

test('cleanup failure downgrades an otherwise passing cell and its count', async () => {
  const fixture = passingFixture();
  fixture.prepareCell = async ({cell, run_id}) => ({...baseBinding(cell, run_id),
    cleanup: async () => { throw Error('owned_cleanup_failed'); }});
  const packet = await runSec01({fixture, source: SOURCE, image: IMAGE,
    runId: 'cleanup-failure', record: async () => {}});
  assert.equal(packet.verdict, 'FAIL');
  assert.equal(packet.results.filter(x => x.status === 'FAIL').length, 30);
  assert.equal(packet.results.filter(x => x.status === 'PASS').length, 0);
  assert.equal(packet.cleanup.filter(x => x.status === 'FAIL').length, 30);
  assert.ok(Object.values(packet.counts).every(count => count.FAIL === 1 && count.PASS === 0));
});
