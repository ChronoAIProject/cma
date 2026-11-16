import test from 'node:test';
import assert from 'node:assert/strict';
import {measuredVisibility} from './enforcement.mjs';

function legacyOracle() {
  let settings = {usage_reporting: {measured_visible: false}}, revision = 1, desired = false;
  const f = {scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, exclusiveSettings: true,
    completedUsage: true, measurementGapMarker: 'Gaps indicate unavailable measurements.', cleanup: async () => {}};
  const actions = [];
  const locator = name => ({waitFor: async () => {}, setChecked: async v => {desired = v;},
    isVisible: async () => name === 'Measured resource charts' ? settings.usage_reporting.measured_visible : true,
    getByText: () => locator('gap'), click: async () => {
      if (name === 'Save settings') {settings.usage_reporting.measured_visible = desired; revision++;}
    }});
  const ctx = {am: 'a', assert: (ok, code) => assert.ok(ok, code), step: async (_name, run) => run(),
    defer: () => {}, observe: async (_label, run) => run(), request: async args => {
      actions.push(args); return {body: {revision, settings: structuredClone(settings)}};
    }, fixture: name => name === 'browser' ? {open: async () => ({origin: 'http://127.0.0.1:19900',
      close: async () => {}, switchAM: async () => {}, page: {goto: async () => {},
        getByRole: (_role, spec) => locator(spec.name), getByLabel: locator, getByText: locator}}),
      evidence: async () => ({requests: [{request_id: 'unrelated', serving_am: 'a'}]})} : {take: async () => f}};
  return {ctx, f, actions};
}

test('a completedUsage boolean cannot substitute for scoped report setup', async () => {
  const h = legacyOracle();
  await assert.rejects(measuredVisibility.run(h.ctx), /admin_measured_scope_fixture/);
  assert.equal(h.actions.length, 0);
});

import {measuredModel} from './measured-model.mjs';
const messages = e => [e.message, ...(e.errors ?? []).map(messages)].join(' ');
const run = h => measuredVisibility.run(h.ctx);

test('both saved modes reopen actual personal/admin reports with scoped inventory and null-gap UI evidence', async () => {
  const h = measuredModel(); await run(h); assert.deepEqual(await h.cleanup(), []);
  assert.deepEqual(h.settings(), h.original);
  const completed = h.observations.filter(o => o.label === 'admin_measured_completed_report').map(o => o.value);
  assert.deepEqual(completed.map(r => [r.actor, r.enabled]), [['user', true], ['admin', true], ['user', false], ['admin', false]]);
  assert.equal(h.sessions.length, 5); assert.ok(h.sessions.every(s => s.closed));
  assert.equal(h.actions.filter(a => a.op === 'report_peer').length, 4);
  assert.ok(h.actions.filter(a => a.op === 'report_peer').every(a => a.am === 'a'));
  const responses = h.observations.filter(o => o.label === 'admin_measured_responses').flatMap(o => o.value.rows);
  assert.ok(responses.length > 0); assert.ok(responses.filter(r => r.path === '/auth/session').every(r => !('email' in r.body)));
  const proof = h.observations.find(o => o.label === 'admin_measured_browser').value.requests[0];
  for (const key of ['serving', 'serving_after', 'started_at', 'ended_at', 'attribution']) assert.ok(proof[key]);
});

for (const [option, code] of [
  ['wrongPrincipal', 'admin_measured_report_scope'], ['incompleteReport', 'admin_measured_report_scope'],
  ['wrongRange', 'admin_measured_report_range'], ['extraPage', 'admin_measured_report_inventory'],
  ['foreignSandbox', 'admin_measured_report_inventory'], ['duplicateSandbox', 'admin_measured_report_inventory'],
  ['wrongSummary', 'admin_measured_summary_visibility'], ['wrongInventoryVisibility', 'admin_measured_inventory_visibility'],
  ['wrongSessionPolicy', 'admin_measured_session_policy'], ['hiddenAllocation', 'admin_measured_allocation_hidden'],
  ['wrongNumbers', 'admin_measured_rendered_allocation'], ['wrongRenderedInventory', 'admin_measured_rendered_inventory'],
  ['peerChanged', 'admin_measured_peer_report'], ['wrongSeriesReport', 'admin_measured_series_identity'],
  ['noGap', 'admin_measured_real_gap_required'], ['wrongGapRow', 'admin_measured_gap_row'],
  ['gapZero', 'admin_measured_gap_fabricated_zero'], ['wrongGapReason', 'admin_measured_gap_reason'],
  ['wrongVisibility', 'admin_measured_visibility_mismatch'], ['wrongReportServing', 'admin_measured_response_ingress'],
  ['unjoinedReport', 'admin_measured_response_ingress'], ['hiddenSeries', 'admin_measured_hidden_series_request'],
  ['lateSeries', 'admin_measured_hidden_series_request'], ['lateWrite', 'admin_measured_unexpected_write'],
  ['changedAllocation', 'admin_measured_toggle_changed_allocations'], ['jsonFailure', 'admin_measured_capture_failed'],
  ['failedSave', 'admin_measured_settings_response'], ['wrongSaveBody', 'admin_measured_settings_body'],
  ['wrongSaveServing', 'admin_measured_settings_ingress'], ['stalePeer', 'admin_measured_peer_policy'],
  ['bearerOnly', 'admin_measured_cookie_session'],
]) test('unproven visibility fails: ' + option, async () => {
  const h = measuredModel({[option]: true});
  await assert.rejects(run(h), e => messages(e).includes(code));
  await h.cleanup();
  assert.ok(h.actions.filter(a => a.op === 'restore').every(a => a.allClosed));
  assert.ok(h.sessions.every(s => s.closed));
});

for (const option of ['preview', 'pendingReport', 'missingDescriptor', 'wrongInventoryReport', 'missingSeries', 'missingAdminCharts']) {
  test('incomplete UI report cannot pass: ' + option, async () => {
    const h = measuredModel({[option]: true, shortBudget: true});
    await assert.rejects(run(h), /admin_measured_report_not_committed|admin_measured_series_missing/);
    await h.cleanup(); assert.ok(h.sessions.every(s => s.closed));
  });
}

test('valid direct POST report responses and a later chart-data page are supported', async () => {
  const h = measuredModel({directReport: true, secondGapPage: true});
  await run(h); assert.deepEqual(await h.cleanup(), []);
  assert.deepEqual(h.actions.filter(a => a.op === 'close' && !a.isSettings).map(a => a.tablePage), [1, 1, 0, 0]);
});

test('initially enabled setting and opposite selected AM both retain exact receipts', async () => {
  const h = measuredModel({initiallyEnabled: true}); h.ctx.am = 'b';
  await run(h); assert.deepEqual(await h.cleanup(), []);
  assert.deepEqual(h.settings(), h.original);
  assert.ok(h.sessions.slice(1).every(s => s.am === 'a'));
  assert.deepEqual(h.actions.filter(a => a.op === 'save').map(a => a.enabled), [false, true]);
});

test('unknown UI save is drained and restored once with no replay', async () => {
  const h = measuredModel({unknownSave: true}); await assert.rejects(run(h), /save_unknown/);
  assert.deepEqual(await h.cleanup(), []); assert.deepEqual(h.settings(), h.original);
  assert.equal(h.actions.filter(a => a.op === 'save').length, 1);
  assert.equal(h.actions.filter(a => a.op === 'restore').length, 1);
  assert.ok(h.actions.filter(a => a.op === 'restore').every(a => a.allClosed));
});

test('cleanup keeps independent close/capture/restoration failures without another write', async () => {
  const h = measuredModel({closeFailure: true, evidenceFailure: true, restoreFailure: true});
  await assert.rejects(run(h)); const errors = await h.cleanup();
  for (const code of ['close_failed', 'evidence_failed', 'restore_unknown'])
    assert.ok(errors.some(e => messages(e).includes(code)), code);
  assert.equal(h.actions.filter(a => a.op === 'restore').length, 1);
  assert.ok(h.actions.filter(a => a.op === 'restore').every(a => a.allClosed));
  assert.ok(!errors.some(e => messages(e).includes('assertion_outside_step')));
});
