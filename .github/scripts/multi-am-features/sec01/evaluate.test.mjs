import test from 'node:test';
import assert from 'node:assert/strict';
import {evaluateProbe, aggregate} from './evaluate.mjs';
import {cells} from './cells.mjs';
import {probeSpecs} from './runner.mjs';
import {baseBinding, observation} from './test-support.mjs';

function example(kind, scenario, positive = false) {
  const cell = cells.find(x => x.category === kind);
  const binding = baseBinding(cell, 'r');
  const spec = probeSpecs(cell, binding).find(x => x.scenario === scenario && (x.expect === 'allow') === positive);
  const context = {run_id: 'r', runtime_id: binding.runtime_id, nonce: binding.nonce, am: 'a',
    owner_tenant: cell.owner_tenant, foreign_tenant: cell.foreign_tenant};
  return {spec, context, observation: observation(spec, binding, context)};
}

test('timeouts and DNS failures cannot be isolated-denial evidence', () => {
  const x = example('network', 'peer_ipv4');
  for (const o of [undefined, {...x.observation, addresses: []}, {...x.observation, addresses: [null]},
    {...x.observation, addresses: [{connected: false, errno: null}]},
    {...x.observation, policy: undefined}, {...x.observation, truncated: true}]) {
    assert.equal(evaluateProbe(x.spec, o, x.context).status, 'BLOCKED');
  }
  assert.equal(evaluateProbe({...x.spec, expect: 'allow'}, x.observation, x.context).status, 'BLOCKED');
  assert.equal(evaluateProbe(x.spec, {...x.observation, failure_class: 'unavailable'}, x.context).status, 'BLOCKED');
});

test('portable timeout and unavailable observations never count as policy denial', () => {
  const x = example('network', 'peer_ipv4');
  for (const row of [
    {family: 'ipv4', connected: false, errno: 110},
    {family: 'ipv4', connected: false, errno: 60},
    {family: 'ipv4', connected: false, errno: 110, failure_class: 'timeout'},
    {family: 'ipv4', connected: false, errno: 60, failure_class: 'timeout'},
    {family: 'ipv4', connected: false, errno: 97, failure_class: 'unavailable'},
  ]) {
    const value = {...x.observation, requested_family: 'ipv4', addresses: [row]};
    assert.equal(evaluateProbe(x.spec, value, x.context).status, 'BLOCKED');
    value.addresses.push({family: 'ipv4', connected: true});
    assert.equal(evaluateProbe(x.spec, value, x.context).status, 'FAIL');
  }
});

test('both IPv6 scenarios require the requested and observed IPv6 family', () => {
  for (const [category, scenario] of [['network', 'peer_ipv6'], ['metadata', 'provider_ipv6_metadata']]) {
    const x = example(category, scenario);
    assert.equal(x.spec.required_family, 'ipv6');
    assert.equal(evaluateProbe(x.spec, x.observation, x.context).status, 'PASS');
    for (const change of [
      o => o.requested_family = 'ipv4', o => delete o.requested_family,
      o => o.addresses[0].family = 'ipv4', o => delete o.addresses[0].family,
      o => o.addresses = [], o => o.addresses[0].failure_class = 'unavailable',
    ]) {
      const value = structuredClone(x.observation); change(value);
      assert.equal(evaluateProbe(x.spec, value, x.context).status, 'BLOCKED');
      value.addresses.push({family: 'ipv4', connected: true});
      assert.equal(evaluateProbe(x.spec, value, x.context).status, 'FAIL');
    }
    x.observation.addresses[0].connected = true;
    assert.equal(evaluateProbe(x.spec, x.observation, x.context).status, 'FAIL');
  }
});

test('every file operation must be observed; even empty forbidden opens fail', () => {
  const x = example('file', 'foreign_home');
  for (const operation of ['read', 'enumerate', 'write_open']) {
    const o = structuredClone(x.observation);
    o.operations.find(x => x.operation === operation).succeeded = true;
    assert.equal(evaluateProbe(x.spec, o, x.context).status, 'FAIL');
  }
  for (const operations of [[], [null], x.observation.operations.slice(1), [{operation: 'read'}]]) {
    assert.equal(evaluateProbe(x.spec, {...x.observation, operations}, x.context).status, 'BLOCKED');
  }
  assert.equal(evaluateProbe(x.spec, {...x.observation, policy: undefined}, x.context).status, 'BLOCKED');
  const owner = example('file', 'owner_canary_read', true);
  assert.equal(evaluateProbe(owner.spec, owner.observation, owner.context).status, 'PASS');
  owner.observation.operations[0].sha256 = 'mismatch';
  assert.equal(evaluateProbe(owner.spec, owner.observation, owner.context).status, 'BLOCKED');
});

for (const [name, error] of [
  ['Linux timeout', {errno: 110, failure_class: 'timeout'}],
  ['macOS timeout', {errno: 60, failure_class: 'timeout'}],
  ['I/O error', {errno: 5, failure_class: 'error'}],
  ['unavailable storage', {errno: 19, failure_class: 'unavailable'}],
  ['zero errno', {errno: 0, failure_class: 'permission_denied'}],
  ['missing classification', {errno: 13, failure_class: undefined}],
  ['null classification', {errno: 13, failure_class: null}],
  ['malformed classification', {errno: 13, failure_class: ['permission_denied']}],
  ['TCP refusal classification', {errno: 13, failure_class: 'refused'}],
  ['unknown classification', {errno: 13, failure_class: 'unknown'}],
]) test(`file ${name} cannot qualify as isolation denial`, () => {
  const x = example('file', 'foreign_home');
  for (const operation of ['read', 'enumerate', 'write_open']) {
    const value = structuredClone(x.observation);
    value.operations = value.operations.map(row => ({...row, failure_class: 'permission_denied',
      ...(row.operation === operation ? error : {})}));
    assert.equal(evaluateProbe(x.spec, value, x.context).status, 'BLOCKED', operation);
    value.operations.find(row => row.operation === operation).succeeded = true;
    delete value.policy;
    assert.equal(evaluateProbe({...x.spec, owner_control: undefined}, value, x.context).status, 'FAIL');
  }
});

test('file ENOENT requires an explicit correlated namespace denial', () => {
  const x = example('file', 'foreign_home');
  x.observation.operations = x.observation.operations.map(row => ({...row, errno: 2, failure_class: 'not_found'}));
  assert.equal(evaluateProbe(x.spec, x.observation, x.context).status, 'BLOCKED');
  x.observation.policy.basis = 'isolated_namespace';
  assert.equal(evaluateProbe(x.spec, x.observation, x.context).status, 'PASS');
  x.observation.policy.run_id = 'unrelated-run';
  assert.equal(evaluateProbe(x.spec, x.observation, x.context).status, 'BLOCKED');
});

test('classified file permission denial still requires nonzero errno and independent evidence', () => {
  const x = example('file', 'foreign_home');
  for (const errno of [1, 13]) {
    x.observation.operations = x.observation.operations.map(row => ({...row, errno, failure_class: 'permission_denied'}));
    assert.equal(evaluateProbe(x.spec, x.observation, x.context).status, 'PASS');
    assert.equal(evaluateProbe(x.spec, {...x.observation, policy: undefined}, x.context).status, 'BLOCKED');
    assert.equal(evaluateProbe({...x.spec, owner_control: undefined}, x.observation, x.context).status, 'BLOCKED');
  }
  for (const errno of [undefined, null, '13', -1, 1.5]) {
    const value = structuredClone(x.observation); value.operations[0].errno = errno;
    assert.equal(evaluateProbe(x.spec, value, x.context).status, 'BLOCKED');
  }
});

test('unavailable or invalid functional dependencies block; TCP alone never substitutes', () => {
  const x = example('network', 'model_canary', true);
  for (const status of [429, 500, 503, 0, 403, undefined]) {
    assert.equal(evaluateProbe(x.spec, {...x.observation, status}, x.context).status, 'BLOCKED');
  }
  assert.equal(evaluateProbe(x.spec, {...x.observation, body_sha256: 'bad'}, x.context).status, 'BLOCKED');
  assert.equal(evaluateProbe(x.spec, {...x.observation, functional_operation: 'wrong'}, x.context).status, 'BLOCKED');
});

test('HTTP negatives require typed refusal with measured private-byte counters', () => {
  const x = example('network', 'redirect_destination');
  for (const change of [o => o.private_bytes = 1, o => o.private_fields = 1,
    o => o.destination_connected = true, o => o.status = 200]) {
    const o = structuredClone(x.observation); change(o);
    assert.equal(evaluateProbe(x.spec, o, x.context).status, 'FAIL');
  }
  for (const change of [o => delete o.private_bytes, o => o.private_fields = -1,
    o => o.code = 'unexpected_code', o => o.status = 302, o => delete o.policy]) {
    const o = structuredClone(x.observation); change(o);
    assert.equal(evaluateProbe(x.spec, o, x.context).status, 'BLOCKED');
  }
});

for (const [category, scenario] of [['network', 'redirect_destination'],
  ['network', 'proxy_direct'], ['metadata', 'metadata_id']]) {
  test(`${scenario} requires an observed absence of a destination connection`, () => {
    const x = example(category, scenario);
    for (const value of [undefined, null, 'unknown', 'false', 0, {}]) {
      const o = {...x.observation, destination_connected: value};
      if (value === undefined) delete o.destination_connected;
      assert.equal(evaluateProbe(x.spec, o, x.context).status, 'BLOCKED');
    }
    assert.equal(evaluateProbe(x.spec, {...x.observation, destination_connected: false}, x.context).status, 'PASS');
    const connected = {...x.observation, destination_connected: true, policy: undefined, private_bytes: undefined};
    assert.equal(evaluateProbe({...x.spec, owner_control: undefined}, connected, x.context).status, 'FAIL');
  });
}

test('HTTP and credential refusals require independent target existence', () => {
  for (const [category, scenario] of [['network', 'redirect_destination'],
    ['network', 'proxy_direct'], ['metadata', 'metadata_id'], ['credential', 'foreign_locator']]) {
    const x = example(category, scenario);
    assert.equal(evaluateProbe(x.spec, x.observation, x.context).status, 'PASS');
    for (const change of [s => delete s.owner_control, s => s.owner_control.exists = false,
      s => s.owner_control.tenant = x.context.owner_tenant, s => s.owner_control.run_id = 'other-run',
      s => delete s.owner_control.evidence_sha256]) {
      const spec = structuredClone(x.spec); change(spec);
      assert.equal(evaluateProbe(spec, x.observation, x.context).status, 'BLOCKED', scenario);
      assert.equal(evaluateProbe(spec, {...x.observation, status: 200}, x.context).status, 'FAIL');
    }
  }
});

test('credential leak counters and revocation ordering preserve every negative failure', () => {
  const x = example('credential', 'revoked_credential');
  for (const counter of ['private_bytes', 'private_fields', 'redirect_authorization_count',
    'redirect_cookie_count', 'old_token_reuse_count', 'unauthorized_mint_count']) {
    assert.equal(evaluateProbe(x.spec, {...x.observation, [counter]: 1}, x.context).status, 'FAIL');
  }
  assert.equal(evaluateProbe(x.spec, {...x.observation, status: 200}, x.context).status, 'FAIL');
  for (const change of [o => delete o.receiver_ledger_sha256, o => delete o.issuer_ledger_sha256,
    o => delete o.redirect_cookie_count, o => o.revocation_sequence = o.attempt_sequence,
    o => o.revocation_sequence = 3, o => delete o.revocation_receipt_sha256,
    o => o.attempt_sequence = '2']) {
    const o = structuredClone(x.observation); change(o);
    assert.equal(evaluateProbe(x.spec, o, x.context).status, 'BLOCKED');
  }
});

test('wrong kind or unsupported observation cannot produce PASS', () => {
  const x = example('network', 'peer_ipv4');
  assert.equal(evaluateProbe(x.spec, {...x.observation, kind: 'other'}, x.context).status, 'BLOCKED');
  assert.equal(evaluateProbe({...x.spec, kind: 'other'}, {...x.observation, kind: 'other'}, x.context).status, 'BLOCKED');
  assert.equal(aggregate([]), 'BLOCKED');
  assert.equal(aggregate(['PASS', 'FAIL', 'BLOCKED']), 'FAIL');
});
