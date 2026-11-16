import test from 'node:test';
import assert from 'node:assert/strict';
import {createAdminFixtures, holdParentLease} from './fixture-provider.mjs';

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
