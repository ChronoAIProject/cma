import test from 'node:test';
import assert from 'node:assert/strict';
import {createAdminFixtures, holdParentLease} from './fixture-provider.mjs';
import {fixturesForCase} from '../../setups.mjs';
import {cases, setups} from './index.mjs';
import {publicSetups} from './public-setups.mjs';

const image = 'localhost/api@sha256:' + '1'.repeat(64);
const config = {scope: 'LOCAL_DISPOSABLE_TWO_AM', database_disposable: true,
  restart_authority: 'PARENT_ONLY', expected_api_image: image, expected_api_source: '2'.repeat(40),
  am_origins: {a: 'http://127.0.0.1:19001', b: 'http://127.0.0.1:19002'}};
const topology = () => ({shared_database: true, coordination_enabled: true,
  ams: {a: {ready: true, image, pod_uid: 'pod-a', instance_id: 'a'},
    b: {ready: true, image, pod_uid: 'pod-b', instance_id: 'b'}}});
function context() {
  const cleanups = [], events = [];
  return {cleanups, events, row: {case_id: 'C275:ADM-044'}, phase: 'before_restart', iteration: 1, am: 'a',
    defer: (label, callback) => cleanups.push(callback),
    observe: async (label, callback) => {const value = await callback(); events.push({label, value}); return value;},
    request: async () => ({status: 200, body: {revision: 1, settings: {retention: {agent_log_days: 30}}}})};
}

test('the real parent callback lease stays held through case cleanup', async () => {
  let held = false, releases = 0;
  const provider = createAdminFixtures({config, adapter: {topology: async () => topology()},
    withExclusiveSettings: async callback => {
      assert.equal(held, false); held = true;
      try {await callback();} finally {held = false; releases++;}
    }});
  const ctx = context(), f = await provider.take('settings_retention', ctx);
  assert.equal(held, true); assert.equal(f.exclusiveSettings, true);
  assert.equal(ctx.events[0].value.topology.ams.a.pod_uid, 'pod-a');
  await f.cleanup(); await f.cleanup();
  assert.equal(held, false); assert.equal(releases, 1);
});

test('unimplemented setup is BLOCKED before acquiring the parent writer lease', async () => {
  let calls = 0;
  const provider = createAdminFixtures({config, adapter: {topology: async () => topology()},
    withExclusiveSettings: async callback => {calls++; await callback();}});
  await assert.rejects(provider.take('organization_history', context()), error =>
    error.name === 'Blocked' && error.message === 'admin_setup_unavailable_organization_history');
  assert.equal(calls, 0);
});

test('a wrong image during setup releases custody and never calls the factory', async () => {
  let held = false, called = false;
  const provider = createAdminFixtures({config,
    adapter: {topology: async () => {const t = topology(); t.ams.b.image = 'wrong'; return t;}},
    withExclusiveSettings: async callback => {held = true; try {await callback();} finally {held = false;}},
    factories: {settings_retention: async () => {called = true; return {cleanup: async () => {}};}}});
  await assert.rejects(provider.take('settings_retention', context()), /coverage_two_ready_exact_image_required/);
  assert.equal(held, false); assert.equal(called, false);
});

test('a timed out queued lease is cancelled before its delayed callback can enter', async () => {
  let later, invoked = 0;
  await assert.rejects(holdParentLease(async callback => {
    await new Promise(resolve => {later = resolve;}); invoked++; await callback();
  }, 5), /admin_parent_lease_timeout/);
  later(); await new Promise(resolve => setImmediate(resolve));
  assert.equal(invoked, 1);
});

test('a missing callback and a nonlocal origin fail closed', async () => {
  await assert.rejects(holdParentLease(async () => {}), /admin_parent_lease_callback_missing/);
  assert.throws(() => createAdminFixtures({config: {...config, am_origins: {
    a: 'https://production.invalid', b: config.am_origins.b}}, adapter: {}}), /admin_parent_nonlocal_origin/);
});

test('registry setup dispatch retains the real adapter lease and advertises only bound recipes', async () => {
  let held = false;
  const provider = createAdminFixtures({config, adapter: {topology: async () => topology()},
    withExclusiveSettings: async callback => {
      held = true; try {await callback();} finally {held = false;}
    }});
  const definition = {...cases.find(c => c.id === 'C275:ADM-044'), owner: 'developer-01'};
  const fixtures = fixturesForCase({setups: new Map([['developer-01', setups]])}, definition,
    {fixtures: provider.capabilities});
  assert.deepEqual(provider.supportedSetups, ['settings_retention', 'spend_period']);
  assert.equal('admin-setup-settings-conflict' in fixtures, false);
  const ctx = {...context(), fixture: name => fixtures[name]};
  const fixture = await fixtures['admin-fixtures'].take('settings_retention', ctx);
  assert.equal(held, true); assert.equal(fixture.exclusiveSettings, true);
  assert.equal(ctx.events.length, 1);
  await fixture.cleanup(); assert.equal(held, false);
});

test('disabled retained budgets and request policies still require their full-settings writers', async () => {
  for (const budget of [
    {enabled: false, owners: [], platform: {period_cap: 1000}},
    {enabled: false, owners: [{owner: 'synthetic', policy: {}}]},
    {enabled: false, owners: [], request_models: [{model: 'synthetic'}]},
  ]) {
    const ctx = context();
    ctx.request = async () => ({body: {revision: 1,
      settings: {retention: {agent_log_days: 30}, model_budget: budget},
      model_budget_writes_available: false}});
    await assert.rejects(publicSetups.settings_retention(ctx), /admin_settings_spend_gate_closed/);
  }
  const ctx = context();
  ctx.request = async () => ({body: {revision: 1, settings: {retention: {agent_log_days: 30},
    model_budget: {enabled: false, owners: [], request_models: [{model: 'synthetic'}]}},
    model_budget_writes_available: true, model_request_budget_writes_available: false}});
  await assert.rejects(publicSetups.settings_retention(ctx), /admin_settings_request_budget_gate_closed/);
});

test('spend-period setup reads actual writer availability and supplies exact boundary inputs without writes', async () => {
  const calls = [], ctx = context();
  ctx.request = async request => {
    calls.push(request);
    return {body: {revision: 1, settings: {retention: {agent_log_days: 30},
      model_budget: {enabled: false, owners: []}},
      model_budget_writes_available: true}};
  };
  const fixture = await publicSetups.spend_period(ctx);
  assert.equal(fixture.period.period_cap, Number.MAX_SAFE_INTEGER);
  assert.ok(Date.parse(fixture.period.period_end) > Date.parse(fixture.period.period_start));
  assert.ok(fixture.invalidPeriods.some(p => p.period_cap === '9007199254740992'));
  assert.ok(fixture.invalidPeriods.some(p => p.period_start === p.period_end));
  assert.equal(calls.length, 1); assert.equal(calls[0].method, 'GET');
  await fixture.cleanup();
  await assert.rejects(publicSetups.spend_period(context()), /admin_settings_spend_gate_closed/);
  const absent = context();
  absent.request = async () => ({body: {revision: 1, model_budget_writes_available: true,
    settings: {retention: {agent_log_days: 30}}}});
  await assert.rejects(publicSetups.spend_period(absent), /admin_spend_schema_unavailable/);
});
