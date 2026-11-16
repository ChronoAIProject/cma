import test from 'node:test';
import assert from 'node:assert/strict';
import {defaultOwnerPolicy} from './spend-browser.mjs';
import {defaultOwnerPolicyModel} from './default-owner-policy-model.mjs';
const run = h => defaultOwnerPolicy.run(h.ctx);
const messages = e => [e.message, ...(e.errors ?? []).map(messages)].join(' ');

test('default policy applies by owner and reservations survive removal', async () => {
  const h = defaultOwnerPolicyModel(); await run(h); assert.deepEqual(await h.cleanup(), []);
  assert.deepEqual(h.settings(), h.original);
  assert.equal(h.actions.filter(a => a.op === 'close').length, 1);
  assert.equal(h.actions.filter(a => a.op === 'restore').length, 0,
    'explicit removal already restored the original policy');
  assert.equal(h.calls.filter(c => c.method === 'PUT').length, 0,
    'explicit removal leaves no settings restoration write');
  const before = h.observations.find(o => o.label === 'admin_reservations_before').value;
  const after = h.observations.find(o => o.label === 'admin_reservations_after').value;
  assert.deepEqual(after, before);
  const applied = h.observations.find(o => o.label === 'admin_default_policy_admission').value;
  const override = h.observations.find(o => o.label === 'admin_override_policy_admission').value;
  assert.deepEqual(applied.policy, h.f.policy);
  assert.deepEqual(override.policy, h.f.overridePolicy);
  assert.notDeepEqual(applied.policy, override.policy);
  assert.equal(applied.provider_requests, 1); assert.equal(override.provider_requests, 1);
  assert.equal(h.page.listenerCount('request'), 0); assert.equal(h.page.listenerCount('response'), 0);
});

for (const am of ['a', 'b']) test('exact default-policy saves and admissions through ' + am, async () => {
  const h = defaultOwnerPolicyModel(); h.ctx.am = am; await run(h); await h.cleanup();
  const saves = h.receipts.filter(r => r.method === 'PUT');
  assert.equal(saves.length, 2);
  for (const receipt of saves) {
    assert.equal(receipt.am, am); assert.equal(receipt.serving_am, am);
    for (const key of ['serving', 'serving_after', 'started_at', 'ended_at', 'attribution']) assert.ok(receipt[key]);
  }
  const proof = h.observations.find(o => o.label === 'admin_browser_final_evidence').value;
  assert.equal(proof.requests.filter(r => r.method === 'PUT').length, 2);
});

for (const [option, code] of [
  ['wrongBody', 'admin_default_policy_save_body'], ['wrongServing', 'admin_default_policy_ingress'],
  ['unjoined', 'admin_default_policy_ingress'], ['duplicate', 'admin_default_policy_write_count'],
  ['bodyFailure', 'body_failed'], ['responseFailure', 'response_failed'], ['failedSave', 'admin_default_policy_save_response'],
  ['bearer', 'admin_default_policy_cookie_session'], ['closeFailure', 'close_failed'], ['evidenceFailure', 'evidence_failed'],
]) test('unproven default policy fails: ' + option, async () => {
  const h = defaultOwnerPolicyModel({[option]: true});
  await assert.rejects(run(h), e => messages(e).includes(code), code);
  const errors = await h.cleanup();
  if (option === 'closeFailure' || option === 'evidenceFailure')
    assert.ok(errors.some(e => messages(e).includes(code)));
  assert.ok(h.actions.filter(a => a.op === 'restore').every(a => a.closed));
  assert.equal(h.page.listenerCount('request'), 0); assert.equal(h.page.listenerCount('response'), 0);
});

for (const [option, code] of [
  ['missingOverride', 'admin_default_budget_fixture_missing'], ['missingOwners', 'admin_default_budget_owners_missing'],
  ['emptyReservations', 'admin_existing_reservations_missing'], ['initialDefault', 'admin_default_policy_absent_fixture_required'],
  ['disabledPolicy', 'admin_default_policy_enabled_fixture_required'],
  ['reservationEffect', 'admin_default_policy_reservations_erased'], ['wrongAdmission', 'admin_default_policy_scope'],
  ['wrongOverrideAdmission', 'admin_default_policy_override_scope'], ['wrongProviderEffect', 'admin_default_policy_provider_effect'],
  ['wrongRemovedAdmission', 'admin_default_policy_removed_scope'],
]) test('invalid default policy fixture or effect fails closed: ' + option, async () => {
  let h = defaultOwnerPolicyModel();
  if (option === 'missingOverride') delete h.f.overridePolicy;
  if (option === 'missingOwners') {h.f.ownerWithoutOverride = undefined; h.f.ownerWithOverride = undefined;}
  if (option === 'emptyReservations') h.f.readReservations = async () => ({items: []});
  if (option === 'initialDefault') h = defaultOwnerPolicyModel({initialDefault: true});
  if (option === 'disabledPolicy') h = defaultOwnerPolicyModel({enabled: false});
  if (option === 'reservationEffect') h = defaultOwnerPolicyModel({reservationEffect: true});
  if (option === 'wrongAdmission') h.f.admitModelRequest = async () => ({status: 201, owner: h.f.ownerWithoutOverride,
    reservation: {owner: h.f.ownerWithoutOverride, policy: h.f.overridePolicy}, policy: h.f.overridePolicy, provider_requests: 1});
  if (option === 'wrongOverrideAdmission') h.f.admitModelRequest = async (_ctx, input) => {
    const owner = typeof input === 'string' ? input : input.owner;
    const selected = owner === h.f.ownerWithoutOverride ? h.f.policy : h.f.policy;
    return {status: 201, owner, reservation: {owner, policy: selected}, policy: selected, provider_requests: 1};
  };
  if (option === 'wrongProviderEffect') h.f.admitModelRequest = async (_ctx, input) => {
    const owner = typeof input === 'string' ? input : input.owner;
    const selected = owner === h.f.ownerWithOverride ? h.f.overridePolicy : h.f.policy;
    return {status: 201, owner, reservation: {owner, policy: selected}, policy: selected, provider_requests: 0};
  };
  if (option === 'wrongRemovedAdmission') h.f.admitModelRequest = async (_ctx, input) => {
    const owner = typeof input === 'string' ? input : input.owner;
    if (h.settings().model_budget.default_policy === undefined)
      return {status: 201, owner, reservation: {owner, policy: h.f.policy}, policy: h.f.policy, provider_requests: 1};
    const selected = owner === h.f.ownerWithOverride ? h.f.overridePolicy : h.f.policy;
    return {status: 201, owner, reservation: {owner, policy: selected}, policy: selected, provider_requests: 1};
  };
  await assert.rejects(run(h), new RegExp(code)); await h.cleanup();
});
