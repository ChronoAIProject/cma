import test from 'node:test';
import assert from 'node:assert/strict';
import {cells, CELL_COUNT, CATEGORIES, STATES, DIRECTIONS} from './cells.mjs';
import {evaluateProbe} from './evaluate.mjs';
import {probeSpecs, runSec01} from './runner.mjs';

import {HASH, SOURCE, IMAGE, OPTIONS, baseBinding, passingFixture} from './test-support.mjs';

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
  const packet = await runSec01({...OPTIONS,fixture: {}, source: SOURCE, image: IMAGE, runId: 'missing-hooks', record: async () => {}});
  assert.equal(packet.runtime_cells, 30);
  assert.equal(packet.results.length, 30);
  assert.equal(packet.verdict, 'BLOCKED');
  assert.equal(packet.results.filter(x => x.status === 'BLOCKED').length, 30);
  assert.equal(packet.results.filter(x => x.status === 'PASS').length, 0);
  for (const cell of cells) assert.deepEqual(packet.counts[cell.id], {PASS: 0, FAIL: 0, BLOCKED: 1, NOT_RUN: 0});
});

test('the complete fixture contract executes every cell and both directions', async () => {
  const packet = await runSec01({...OPTIONS,fixture: passingFixture(), source: SOURCE, image: IMAGE,
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
  const spec = probeSpecs(cell, {targets: {revoked_credential: {denial_code: 'credential_not_authorized'}}}).find(x => x.scenario === 'revoked_credential');
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
  const packet = await runSec01({...OPTIONS,fixture, source: SOURCE, image: IMAGE,
    runId: 'cleanup-failure', record: async () => {}});
  assert.equal(packet.verdict, 'FAIL');
  assert.equal(packet.results.filter(x => x.status === 'FAIL').length, 30);
  assert.equal(packet.results.filter(x => x.status === 'PASS').length, 0);
  assert.equal(packet.cleanup.filter(x => x.status === 'FAIL').length, 30);
  assert.ok(Object.values(packet.counts).every(count => count.FAIL === 1 && count.PASS === 0));
});

test('full campaigns retain timed-out TCP cells as BLOCKED on both AMs', async () => {
  for (const errno of [110, 60]) {
    const fixture = passingFixture(), probe = fixture.runtimeProbe;
    fixture.runtimeProbe = async args => {
      const value = await probe(args);
      if (value.kind === 'tcp') value.addresses = value.addresses.map(row =>
        ({...row, errno, failure_class: 'timeout'}));
      return value;
    };
    const packet = await runSec01({...OPTIONS, fixture});
    assert.equal(packet.verdict, 'BLOCKED');
    assert.equal(packet.results.filter(row => row.status === 'BLOCKED').length, 18);
    assert.equal(packet.results.filter(row => row.status === 'PASS').length, 12);
    assert(packet.results.every(row => row.cleanup === 'PASS'));
  }
});

test('full campaigns cannot qualify IPv6 using IPv4 or unavailable observations', async () => {
  for (const unavailable of [false, true]) {
    const fixture = passingFixture(), probe = fixture.runtimeProbe;
    fixture.runtimeProbe = async args => {
      const value = await probe(args);
      if (['peer_ipv6', 'provider_ipv6_metadata'].includes(args.spec.scenario)) {
        value.addresses = value.addresses.map(row => unavailable
          ? {...row, failure_class: 'unavailable'} : {...row, family: 'ipv4'});
      }
      return value;
    };
    const packet = await runSec01({...OPTIONS, fixture});
    assert.equal(packet.verdict, 'BLOCKED');
    assert.equal(packet.results.filter(row => row.status === 'BLOCKED').length, 12);
    for (const row of packet.results.filter(row => ['network', 'metadata'].includes(row.category))) {
      assert(row.outcomes.filter(x => x.status === 'BLOCKED').some(x => x.am === 'a'));
      assert(row.outcomes.filter(x => x.status === 'BLOCKED').some(x => x.am === 'b'));
    }
  }
});
