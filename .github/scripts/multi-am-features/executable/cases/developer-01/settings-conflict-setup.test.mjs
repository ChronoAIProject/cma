import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {createAdminFixtures} from './fixture-provider.mjs';

const defaults = JSON.parse(readFileSync(new URL('./reset-contract.json', import.meta.url))).defaults;
const image = 'localhost/api@sha256:' + '1'.repeat(64);
function fixture({writer = true, enabled = false} = {}) {
  const calls = [], cleanups = [];
  const settings = structuredClone(defaults); settings.model_budget.enabled = enabled;
  const config = {scope: 'LOCAL_DISPOSABLE_TWO_AM', database_disposable: true,
    restart_authority: 'PARENT_ONLY', expected_api_source: '2'.repeat(40), expected_api_image: image,
    am_origins: {a: 'http://127.0.0.1:19001', b: 'http://127.0.0.1:19002'}};
  const ctx = {row: {case_id: 'C275:ADM-041'}, phase: 'before_restart', iteration: 1, am: 'a',
    defer: (_label, callback) => cleanups.push(callback), observe: async (_label, callback) => callback(),
    request: async request => {
      calls.push(request); assert.equal(request.method, 'GET');
      return {body: {revision: 1, settings: structuredClone(settings), model_budget_writes_available: writer}};
    }};
  const provider = createAdminFixtures({config, withExclusiveSettings: callback => callback(),
    adapter: {topology: async () => ({shared_database: true, coordination_enabled: true,
      ams: {a: {ready: true, image, pod_uid: 'a', instance_id: 'a'},
        b: {ready: true, image, pod_uid: 'b', instance_id: 'b'}}})}});
  return {calls, ctx, provider, settings,
    cleanup: async () => {for (const fn of cleanups.toReversed()) await fn();}};
}

test('the public settings-conflict recipe supplies distinct disabled policies without setup writes', async () => {
  const f = fixture();
  try {
    const setup = await f.provider.take('settings_conflict', f.ctx);
    assert.equal(setup.exclusiveSettings, true);
    assert.equal(typeof setup.faults.withholdNextSuccessfulResponse, 'function');
    await assert.rejects(setup.faults.withholdNextSuccessfulResponse({method: 'PUT',
      path: '/api/v1/admin/settings', session: {}}), /admin_fault_browser_binding_missing/);
    const lists = await setup.concurrentLists(f.ctx);
    assert.equal(new Set([lists.initial, lists.server, lists.draft].map(b => b.owners[0].owner)).size, 3);
    for (const budget of [lists.initial, lists.server, lists.draft]) {
      assert.equal(budget.enabled, false);
      assert.ok(budget.owners[0].policy.max_unobserved_units < budget.owners[0].policy.turn_limit);
      assert.ok(budget.owners[0].policy.turn_limit <= budget.owners[0].policy.owner_day_limit);
      assert.deepEqual({...budget, owners: []}, {...f.settings.model_budget, owners: []});
    }
    lists.initial.owners[0].policy.turn_limit++;
    assert.equal(lists.server.owners[0].policy.turn_limit, 100);
    assert.equal(lists.draft.owners[0].policy.turn_limit, 100);
    await lists.cleanup();
    assert.ok(f.calls.length >= 2); assert.ok(f.calls.every(call => call.method === 'GET'));
  } finally {await f.cleanup();}
});

test('active background systems, absent policy fields and unavailable retained writers block setup', async () => {
  const variants = [
    [s => {s.inactivity.auto_pause_enabled = true;}, /disabled_background_policies/],
    [s => {s.triggers.enabled = true;}, /disabled_background_policies/],
    [s => {delete s.inactivity;}, /disabled_background_policies/],
    [s => {delete s.model_budget;}, /disabled_background_policies/],
    [s => {s.model_budget.platform = {cloud: {enabled: false}};}, /cloud_writer_unavailable/],
    [s => {s.model_budget.request_models = [{model: 'fixture-model'}];}, /request_budget_gate_closed/],
  ];
  for (const [mutate, reason] of variants) {
    const f = fixture(); mutate(f.settings);
    try {await assert.rejects(f.provider.take('settings_conflict', f.ctx), reason);}
    finally {await f.cleanup();}
  }
});

test('list inputs reread actual policy state and refuse newly active enforcement', async () => {
  const f = fixture();
  try {
    const setup = await f.provider.take('settings_conflict', f.ctx);
    f.settings.model_budget.enabled = true;
    await assert.rejects(setup.concurrentLists(f.ctx), /disabled_background_policies/);
    assert.equal(f.calls.length, 2); assert.ok(f.calls.every(call => call.method === 'GET'));
  } finally {await f.cleanup();}
});

test('the conflict setup blocks unavailable writers and active budget policies', async () => {
  for (const [options, reason] of [[{writer: false}, /admin_settings_spend_gate_closed/],
    [{enabled: true}, /admin_conflict_requires_disabled_background_policies/]]) {
    const f = fixture(options);
    try {await assert.rejects(f.provider.take('settings_conflict', f.ctx), reason);}
    finally {await f.cleanup();}
  }
});
