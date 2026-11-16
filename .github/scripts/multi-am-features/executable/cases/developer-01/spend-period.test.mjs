import test from 'node:test';
import assert from 'node:assert/strict';
import {spendPeriod} from './spend-browser.mjs';
import {periodModel} from './spend-period-model.mjs';
import {withPrefetch} from './operations-prefetch-model.mjs';
import {invalidPlatformPeriods} from './spend-period-data.mjs';
const run = h => spendPeriod.run(h.ctx);
const messages = e => [e.message, ...(e.errors ?? []).map(messages)].join(' ');

test('spend period closes before restoring original policy', async () => {
  const h = periodModel(); await run(h); await h.cleanup();
  assert.ok(h.actions.filter(a => a.op === 'restore').every(a => a.closed));
});
test('an unrelated field error is not proof that the intended invalid field was rejected', async () => {
  const h = periodModel({wrongError: true});
  await assert.rejects(run(h), e => messages(e).includes('admin_period_field_error')); await h.cleanup();
});
test('server refusal cannot substitute for rejecting an invalid form before dispatch', async () => {
  const h = periodModel({invalidDispatch: true});
  await assert.rejects(run(h), e => messages(e).includes('admin_period_extra_write')); await h.cleanup();
});

test('exact accounting units, fifteen field errors and both-AM snapshots retain full serving proof', async () => {
  const h = periodModel(); delete h.f.invalidPeriods;
  await run(h); assert.deepEqual(await h.cleanup(), []); assert.deepEqual(h.settings(), h.original);
  assert.equal(h.actions.filter(a => a.op === 'click' && a.invalid).length, 15);
  assert.equal(h.receipts.filter(r => r.method === 'PUT').length, 1);
  const saved = h.observations.find(o => o.label === 'admin_period_save').value;
  assert.equal(saved.body.settings.model_budget.platform.period_cap, Number.MAX_SAFE_INTEGER);
  assert.equal(saved.body.revision, 2);
  const raw = h.observations.filter(o => o.label === 'admin_period_browser').at(-1).value.requests.at(-1);
  for (const key of ['serving', 'serving_after', 'started_at', 'ended_at', 'attribution']) assert.ok(raw[key]);
  assert.equal(raw.request_id, saved.request_id);
  const problems = h.observations.filter(o => o.label === 'admin_period_field_error').map(o => o.value.text);
  assert.deepEqual(problems, invalidPlatformPeriods(h.f.period).map(p => p.error));
  assert.equal(h.calls.filter(c => c.method === 'PUT').length, 1);
  assert.deepEqual(new Set(h.calls.map(c => c.am)), new Set(['a', 'b']));
  assert.equal(h.page.listenerCount('request'), 0); assert.equal(h.page.listenerCount('response'), 0);
});

test('existing optional cloud policy survives exact period replacement and restoration on the other AM', async () => {
  const h = periodModel({existing: true}); h.ctx.am = 'b'; h.f.period.period_cap--;
  await run(h); assert.deepEqual(await h.cleanup(), []); assert.deepEqual(h.settings(), h.original);
  const saved = h.observations.find(o => o.label === 'admin_period_save').value;
  assert.deepEqual(saved.body.settings.model_budget.platform.cloud, h.original.model_budget.platform.cloud);
  assert.equal(saved.body.settings.model_budget.platform.period_cap, Number.MAX_SAFE_INTEGER - 1);
  assert.ok(h.receipts.filter(r => r.method === 'PUT').every(r => r.am === 'b' && r.serving_am === 'b'));
});

for (const [option, code] of [
  ['rounded', 'admin_period_exact_draft'], ['errorMissing', 'admin_period_field_error'],
  ['wrongBody', 'admin_period_save_body'], ['failedSave', 'admin_period_save_response'],
  ['wrongServing', 'admin_period_save_ingress'], ['unjoined', 'admin_period_save_ingress'],
  ['badRevision', 'admin_period_saved_policy'], ['stalePeer', 'admin_period_peer_policy'],
  ['erasedOther', 'admin_period_saved_policy'], ['lateWrite', 'admin_period_extra_write'],
  ['bearer', 'admin_period_cookie_session'], ['writerMissing', 'admin_period_writer_fixture'],
  ['invalidEffect', 'admin_period_invalid_effect'],
]) test('unproven spend period fails: ' + option, async () => {
  const h = periodModel({[option]: true}); await assert.rejects(run(h), e => messages(e).includes(code));
  await h.cleanup(); assert.ok(h.actions.filter(a => a.op === 'restore').every(a => a.closed));
  assert.equal(h.page.listenerCount('request'), 0); assert.equal(h.page.listenerCount('response'), 0);
});

for (const options of [{}, {late: true}, {status: 200}]) test('legitimate report prefetch is retained separately: ' + JSON.stringify(options), async () => {
  const h = periodModel(); withPrefetch(h, options); await run(h); assert.deepEqual(await h.cleanup(), []);
  const raw = h.observations.filter(o => o.label === 'admin_period_browser').at(-1).value;
  assert.equal(raw.requests.filter(r => r.method === 'POST').length, 1);
  assert.equal(raw.requests.filter(r => r.method === 'PUT').length, 1);
});

test('unknown UI save is not retried and restores the observed intended policy after drain', async () => {
  const h = periodModel({unknownSave: true}); await assert.rejects(run(h), /save_unknown/);
  assert.deepEqual(await h.cleanup(), []); assert.deepEqual(h.settings(), h.original);
  assert.equal(h.receipts.filter(r => r.method === 'PUT').length, 1);
  assert.equal(h.actions.filter(a => a.op === 'restore').length, 1);
});

test('unexpected invalid acceptance fails and the negative intent retains cleanup custody', async () => {
  const h = periodModel({invalidAccepted: true}); await assert.rejects(run(h), /admin_period_extra_write/);
  assert.ok((await h.cleanup()).some(e => messages(e).includes('admin_period_extra_write')));
  assert.deepEqual(h.settings(), h.original);
  assert.equal(h.actions.filter(a => a.op === 'click').length, 1);
  assert.equal(h.actions.filter(a => a.op === 'restore').length, 1);
});

test('close, evidence and unknown restoration failures remain independent without replay', async () => {
  const h = periodModel({closeFailure: true, evidenceFailure: true, restoreFailure: true});
  await assert.rejects(run(h), e => messages(e).includes('close_failed'));
  const errors = await h.cleanup();
  for (const code of ['close_failed', 'evidence_failed', 'restore_unknown']) assert.ok(errors.some(e => messages(e).includes(code)));
  assert.ok(!errors.some(e => messages(e).includes('assertion_outside_step')));
  assert.equal(h.actions.filter(a => a.op === 'restore').length, 1);
  assert.ok(h.actions.filter(a => a.op === 'restore').every(a => a.closed));
});

test('unknown restoration remains sticky through every cleanup hook', async () => {
  const h = periodModel({restoreFailure: true}); await assert.rejects(run(h), e => messages(e).includes('restore_unknown'));
  assert.ok((await h.cleanup()).some(e => messages(e).includes('restore_unknown')));
  assert.equal(h.actions.filter(a => a.op === 'restore').length, 1);
});

for (const invalid of [
  null, {period_cap: 1000}, {period_cap: '9007199254740991'}, {warning_percent: 0}, {stop_percent: 101},
  {warning_percent: 80}, {protected_reserve: -1}, {lag_allowance: 1.5}, {unrecognized: true},
  {protected_reserve: Number.MAX_SAFE_INTEGER}, {period_start: '2026-02-30T00:00:00Z'},
  {period_end: '2026-11-17T00:00:00.000Z'},
]) test('invalid or insufficient exactness fixture blocks before browser writes: ' + JSON.stringify(invalid), async () => {
  const h = periodModel(); h.f.period = invalid === null ? null : {...h.f.period, ...invalid};
  await assert.rejects(run(h), /admin_period_(fixture_shape|exact_integer_fixture|reserve_fixture|time_fixture)/);
  assert.deepEqual(await h.cleanup(), []); assert.equal(h.calls.length, 0); assert.equal(h.actions.length, 0);
});

test('negative-case coverage cannot be reduced by fixture-provided variants', () => {
  const h = periodModel(); h.f.invalidPeriods = [];
  const rows = invalidPlatformPeriods(h.f.period);
  assert.equal(rows.length, 15);
  assert.ok(rows.some(r => r.field === 'period_cap' && r.value.period_cap === '9007199254740992'));
  assert.ok(rows.some(r => r.field === 'period_end' && r.value.period_end === r.value.period_start));
  assert.ok(rows.some(r => r.field === 'protected_reserve'));
});
