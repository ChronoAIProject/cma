import test from 'node:test';
import assert from 'node:assert/strict';
import {spendAvailability} from './spend-browser.mjs';
import {availabilityModel} from './spend-availability-model.mjs';
import {withPrefetch} from './operations-prefetch-model.mjs';
const run = h => spendAvailability.run(h.ctx);
const messages = e => [e.message, ...(e.errors ?? []).map(messages)].join(' ');

test('availability policy restoration follows browser drain', async () => {
  const h = availabilityModel(); await run(h); await h.cleanup();
  assert.ok(h.actions.filter(a => a.op === 'restore').every(a => a.closed));
});
test('availability recovery is attempted once across normal and deferred cleanup', async () => {
  const h = availabilityModel(); await run(h); await h.cleanup();
  assert.deepEqual(h.actions.filter(a => a.op === 'writer').map(a => a.value), [false, true]);
});
test('an unrelated 503 cannot prove the spend writer refusal', async () => {
  const h = availabilityModel({wrongCode: true});
  await assert.rejects(run(h), e => messages(e).includes('admin_spend_availability_refusal'));
  await h.cleanup();
});

for (const enabled of [false, true]) for (const am of ['a', 'b'])
test(`exact toggle ${enabled} through ${am} retains policies, typed refusal and serving proof`, async () => {
  const h = availabilityModel({enabled}); h.ctx.am = am;
  await run(h); assert.deepEqual(await h.cleanup(), []); assert.deepEqual(h.settings(), h.original);
  const writes = h.observations.filter(o => o.label === 'admin_spend_availability_writes').at(-1).value;
  assert.equal(writes.length, 2); assert.deepEqual(writes.map(w => w.response.status), [503, 200]);
  assert.equal(writes[0].response.body.code, 'model_budget_writes_unavailable');
  assert.deepEqual(writes[0].body, writes[1].body);
  assert.equal(writes[0].body.expected_revision, 1);
  assert.deepEqual(writes[0].body.settings, {...h.original, model_budget: {...h.original.model_budget, enabled: !enabled}});
  const raw = h.observations.filter(o => o.label === 'admin_spend_availability_browser').at(-1).value.requests;
  for (const r of raw.filter(r => r.method === 'PUT')) {
    assert.equal(r.am, am); assert.equal(r.serving_am, am);
    for (const field of ['serving', 'serving_after', 'started_at', 'ended_at', 'attribution']) assert.ok(r[field]);
  }
  assert.deepEqual(new Set(h.calls.map(c => c.am)), new Set(['a', 'b']));
  assert.equal(h.actions.filter(a => a.op === 'restore').length, 1);
  assert.equal(h.actions.filter(a => a.op === 'submit' && a.blocked).length, 1);
  assert.equal(h.page.listenerCount('request'), 0); assert.equal(h.page.listenerCount('response'), 0);
});

for (const options of [{noDefault: true}, {noOwners: true}, {noModels: true}])
test('supported absent retained section is still proved: ' + JSON.stringify(options), async () => {
  const h = availabilityModel(options); await run(h); assert.deepEqual(await h.cleanup(), []);
  assert.deepEqual(h.settings(), h.original);
});

for (const [option, code] of [
  ['wrongStatus', 'admin_spend_availability_refusal'], ['wrongBody', 'admin_spend_availability_body'],
  ['wrongServing', 'admin_spend_availability_ingress'], ['unjoined', 'admin_spend_availability_ingress'],
  ['duplicateReceipt', 'admin_spend_availability_uncaptured_mutation'],
  ['responseBodyFailure', 'response_body_failed'], ['requestBodyFailure', 'request_body_failed'],
  ['lostDraft', 'admin_spend_availability_toggle'], ['editableClosed', 'admin_spend_availability_toggle'],
  ['extraGroup', 'admin_spend_availability_list_count'], ['disabledDispatch', 'admin_spend_availability_extra_write'],
  ['modeEffect', 'admin_spend_availability_peer'], ['peerGate', 'admin_spend_availability_peer'],
  ['refusalEffect', 'admin_spend_availability_peer'], ['badRefresh', 'admin_spend_availability_refresh_snapshot'],
  ['erasedOther', 'admin_spend_availability_saved_policy'], ['failedSave', 'admin_spend_availability_save_response'],
  ['stalePeer', 'admin_spend_availability_peer'], ['lateWrite', 'admin_spend_availability_extra_write'],
  ['unresolvedLate', 'admin_spend_availability_extra_write'], ['bearer', 'admin_spend_availability_cookie_session'],
  ['missingDescription', 'admin_spend_availability_scope_text'], ['wrongScope', 'admin_spend_availability_scope_text'],
  ['hiddenScope', 'admin_spend_availability_scope_text'],
]) test('unproven availability cannot pass: ' + option, async () => {
  const h = availabilityModel({[option]: true});
  await assert.rejects(run(h), e => messages(e).includes(code), code); await h.cleanup();
  assert.ok(h.actions.filter(a => a.op === 'restore').every(a => a.closed));
  assert.ok(h.actions.filter(a => a.op === 'writer' && a.value).length <= 1);
  assert.equal(h.page.listenerCount('request'), 0); assert.equal(h.page.listenerCount('response'), 0);
});

for (const field of ['Period start (inclusive, RFC3339)', 'Per-turn cap (accounting units)',
  'Owner override 1: owner ID', 'Owner override 1: Day-boundary timezone (IANA)',
  'Request model 1: model name', 'Request model 1: input overhead (tokens)'])
for (const option of ['hiddenField', 'changedField']) test(`retained field ${option}: ${field}`, async () => {
  const h = availabilityModel({[option]: field});
  await assert.rejects(run(h), /admin_spend_availability_retained_field/); await h.cleanup();
});

for (const options of [{}, {late: true}, {status: 200}]) test('real report-prefetch evidence remains separate: ' + JSON.stringify(options), async () => {
  const h = availabilityModel(); withPrefetch(h, options); await run(h); assert.deepEqual(await h.cleanup(), []);
  const proof = h.observations.filter(o => o.label === 'admin_spend_availability_browser').at(-1).value;
  assert.equal(proof.requests.filter(r => r.method === 'POST').length, 1);
  assert.equal(proof.requests.filter(r => r.method === 'PUT').length, 2);
});

for (const options of [{unresolved: true, late: true}, {status: 503}, {wrongAM: true}])
test('unproven report finality prevents availability acceptance: ' + JSON.stringify(options), async () => {
  const h = availabilityModel(); withPrefetch(h, options);
  await assert.rejects(run(h), e => messages(e).includes('admin_operations_report')); await h.cleanup();
});

for (const [option, code] of [['unknownDisable', 'disable_unknown'], ['unknownRecovery', 'recovery_unknown'],
  ['unknownRefusal', 'refusal_unknown'], ['unknownSave', 'save_unknown']])
test('uncertain operation retains custody without replay: ' + option, async () => {
  const h = availabilityModel({[option]: true}); await assert.rejects(run(h), e => messages(e).includes(code));
  const failures = await h.cleanup();
  if (option === 'unknownRecovery') assert.ok(failures.some(e => messages(e).includes(code)));
  else assert.deepEqual(failures, []);
  assert.deepEqual(h.actions.filter(a => a.op === 'writer').map(a => a.value), [false, true]);
  assert.deepEqual(h.settings(), h.original);
  assert.ok(h.actions.filter(a => a.op === 'restore').every(a => a.closed));
  assert.ok(h.observations.some(o => o.label === 'admin_spend_availability_final' && o.value.am === 'a'));
  assert.ok(h.observations.some(o => o.label === 'admin_spend_availability_final' && o.value.am === 'b'));
});

test('a synchronous uncertain recovery throws only once across cleanup hooks', async () => {
  const h = availabilityModel(), original = h.f.setWriterAvailability;
  h.f.setWriterAvailability = value => {if (!value) return original(value);
    h.actions.push({op: 'writer', value}); throw Error('synchronous_recovery_unknown');};
  await assert.rejects(run(h), /synchronous_recovery_unknown/);
  assert.ok((await h.cleanup()).some(e => messages(e).includes('synchronous_recovery_unknown')));
  assert.equal(h.actions.filter(a => a.op === 'writer' && a.value).length, 1);
});

test('close, capture, restore and both final-read failures retain independent custody', async () => {
  const h = availabilityModel({closeFailure: true, evidenceFailure: true, restoreFailure: true, finalReadFailures: ['a', 'b']});
  await assert.rejects(run(h), e => messages(e).includes('close_failed'));
  const errors = await h.cleanup();
  for (const code of ['close_failed', 'evidence_failed', 'final_read_a', 'final_read_b'])
    assert.ok(errors.some(e => messages(e).includes(code)), code);
  assert.equal(h.actions.filter(a => a.op === 'writer' && a.value).length, 1);
  assert.ok(!errors.some(e => messages(e).includes('assertion_outside_step')));
});

test('unknown restoration is sticky after all cleanup hooks without a second PUT', async () => {
  const h = availabilityModel({restoreFailure: true});
  await assert.rejects(run(h), e => messages(e).includes('restore_unknown'));
  assert.ok((await h.cleanup()).some(e => messages(e).includes('restore_unknown')));
  assert.equal(h.calls.filter(c => c.method === 'PUT').length, 1);
  assert.deepEqual(h.settings(), h.original);
});

for (const options of [{available: false}, {requestAvailable: false}, {noDefault: true, noOwners: true}])
test('insufficient retained/writable fixture blocks before changing writer mode: ' + JSON.stringify(options), async () => {
  const h = availabilityModel(options); await assert.rejects(run(h), /admin_spend_availability_(writable|retained)_fixture/);
  assert.deepEqual(await h.cleanup(), []); assert.equal(h.actions.filter(a => a.op === 'writer').length, 0);
});
