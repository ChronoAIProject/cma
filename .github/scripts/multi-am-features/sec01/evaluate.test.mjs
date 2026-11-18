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
