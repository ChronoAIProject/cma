import test from 'node:test';
import assert from 'node:assert/strict';
import {prepareAdminRoles, prepareBasicUsage, roleCapabilities} from './setups.mjs';
import {reserve, borrowCustody} from './setup-common.mjs';

// A real non-reentrant mutex: another acquisition remains pending until the
// holder releases. Immediate fake reserve responses cannot catch nested locks.
function queuedDatabase(events) {
  let held = false; const waiting = [];
  const db = {scope: 'LOCAL_DISPOSABLE_TWO_AM', database_disposable: true, attempts: 0, releases: 0,
    async reserve({exclusive}) {
      assert.equal(exclusive, true); db.attempts++;
      if (held) await new Promise(resolve => waiting.push(resolve));
      held = true; let released = false;
      return {exclusive: true, parent_cleanup_barrier: true,
        async retainIntent(value) {assert.equal(held, true); events.push(['intent', value.key]);},
        async retainDocument(value) {assert.equal(held, true); events.push(['document', value]);},
        async release() {
          if (!released) {
            released = true; db.releases++; events.push(['release']);
            const next = waiting.shift(); if (next) next(); else held = false;
            if (db.failRelease) throw Error('lease_release_failed');
          }
          return {registered: true, parent_cleanup_barrier: true};
        }};
    }};
  return db;
}
function scenario() {
  const events = [], deferred = [], observations = [], requests = [], records = [];
  const identities = Object.fromEntries(['admin', 'user', 'outsider', ...Object.keys(roleCapabilities)]
    .map(actor => [actor, {actor, subject: 'subject-' + actor}]));
  const grants = new Map(Object.entries(roleCapabilities).map(([actor, capabilities]) => [actor,
    {actor, subject: identities[actor].subject, revision: 1, capabilities: [...capabilities]}]));
  grants.get('read_operator').capabilities.push('configure_platform');
  const db = queuedDatabase(events);
  const auth = {async readGrant({actor}) {
    if (auth.failRestore) throw Error('grant_read_failed');
    return structuredClone(grants.get(actor));
  }, async replaceGrant({actor, expected_revision, capabilities}) {
    assert.equal(grants.get(actor).revision, expected_revision);
    const value = {...grants.get(actor), revision: expected_revision + 1, capabilities};
    grants.set(actor, value); events.push(['grant', actor, [...capabilities]]); return structuredClone(value);
  }};
  let reportActor;
  const ctx = {row: {case_id: 'C275:ADM-003'}, am: 'a',
    defer(label, fn) {deferred.push({label, fn});},
    async observe(label, fn) {const value = await fn(); observations.push({label, value}); return value;},
    async request(input) {
      requests.push(input); const path = new URL(input.path, 'http://local.invalid').pathname;
      let body, status = 200;
      if (path === '/auth/session') body = {authenticated: true, subject: identities[input.actor].subject,
        capabilities: grants.get(input.actor).capabilities};
      else if (path === '/api/v1/sandboxes' && input.method === 'POST') {
        if (ctx.failCreate) throw Error('create_outcome_unknown');
        const value = {id: 'sandbox-' + (records.length + 1), actor: input.actor, status: 'active'};
        records.push(value); body = {id: value.id}; status = 201;
        if (ctx.failAttribution) {const error = Error('request_attribution_missing'); error.response = {body, status}; throw error;}
      } else if (path === '/api/v1/admin/usage/sandboxes') body = {items: records.map(r => ({
        sandbox_id: r.id, owner: identities[r.actor].subject, tracking_started_at: 100,
        complete_lifecycle: true, allocation: {running: false, cpu_millis: 0, memory_mib: 0, disk_gib: 0},
      })), next: null};
      else if (path.endsWith('/events')) body = {items: [{revision: 1, started_at: 100, ended_at: 101,
        allocation: {running: false, cpu_millis: 0, memory_mib: 0, disk_gib: 0},
        next_allocation: {running: false, cpu_millis: 0, memory_mib: 0, disk_gib: 0}}], next: null};
      else if (path === '/api/v1/admin/usage/metrics/reports') {
        reportActor = input.actor; body = {report_id: 'report-one'}; status = 202;
      } else if (path.endsWith('/reports/report-one')) {
        assert.equal(input.actor, reportActor);
        body = {report_id: 'report-one', status: 'ready', report_complete: true,
          scope: {admin: true, principal: identities[input.actor].subject}, summary: {sandbox_count: 2,
            allocated_period: {cpu_core_hours: 0, memory_gib_hours: 0, disk_gib_hours: 0}}};
      } else {
        const value = records.find(r => path.includes('/' + r.id)); assert.ok(value, path);
        if (input.method === 'DELETE') {
          events.push(['retire', value.id]);
          if (ctx.failRetire) throw Error('retirement_failed');
          value.status = 'retired';
        }
        if (path.endsWith('/pause')) {value.status = 'paused'; status = 202;}
        body = {id: value.id, status: value.status};
      }
      return {status, body, timing: {am: input.am ?? 'a', request_id: 'receipt-' + requests.length}};
    }};
  return {ctx, db, auth, events, deferred, observations, requests, grants, primitives: {
    auth, identities, disposableDatabase: db, usageRuntime: {sandbox_profile: {id: 'profile', revision: 1},
      async finality({sandboxId}) {return {sandbox_id: sandboxId, runtime_absent: true, home_absent: true};}},
  }};
}

test('ADM003 completes using one held mutex and restores before the queued contender runs', {timeout: 2000}, async () => {
  const s = scenario(); const f = await prepareAdminRoles(s.ctx, s.primitives);
  assert.equal(s.db.attempts, 1); assert.equal(s.db.releases, 0);
  assert.equal(s.requests.find(r => r.path.endsWith('/metrics/reports')).actor, 'read_operator');
  assert.equal(f.sandbox_id, 'sandbox-1'); assert.equal(f.report_id, 'report-one');
  let acquired = false;
  const pending = s.db.reserve({exclusive: true}).then(lease => {acquired = true; s.events.push(['contender']); return lease;});
  await new Promise(resolve => setImmediate(resolve)); assert.equal(acquired, false);
  await f.cleanup(); const contender = await pending;
  assert.equal(acquired, true);
  assert.deepEqual(s.grants.get('read_operator').capabilities, ['read_all_sandboxes', 'configure_platform']);
  const releaseIndex = s.events.findIndex(e => e[0] === 'release');
  assert.equal(s.events.slice(0, releaseIndex).filter(e => e[0] === 'retire').length, 2);
  assert.equal(s.events[releaseIndex - 1][0], 'grant');
  await contender.release();
  for (const {fn} of s.deferred.reverse()) await fn();
  assert.equal(s.db.releases, 2, 'one outer release and one contender release');
});

test('resource, grant and release failures remain distinct and cleanup does not retry after unlock', async () => {
  const s = scenario(); const f = await prepareAdminRoles(s.ctx, s.primitives);
  s.ctx.failRetire = true; s.auth.failRestore = true; s.db.failRelease = true;
  const error = await f.cleanup().catch(error => error);
  assert.deepEqual(error.errors.map(e => e.message), ['usage_resources', 'auth_grants', 'auth_lease_release']);
  assert.equal(error.errors[1].cause.message, 'admin09_auth_restore_failed');
  assert.equal(error.errors[2].cause.message, 'lease_release_failed');
  const requestCount = s.requests.length;
  for (const {fn} of s.deferred.reverse()) await assert.rejects(fn());
  assert.equal(s.requests.length, requestCount); assert.equal(s.db.releases, 1);
  assert.ok(s.observations.some(o => o.value.stage === 'auth_grants'));
  assert.ok(s.observations.some(o => o.value.stage === 'auth_lease_release'));
});

test('unknown creation is never retried and partial usage cleanup cannot release outer custody early', async () => {
  const s = scenario(); s.ctx.failCreate = true;
  await assert.rejects(prepareAdminRoles(s.ctx, s.primitives), /create_outcome_unknown/);
  const failures = [];
  for (const {fn} of s.deferred.reverse()) {try {await fn();} catch (error) {failures.push(error);}}
  assert.ok(failures.some(e => e.errors?.some(e => e.cause?.message === 'usage_setup_cleanup_failed')));
  assert.equal(s.requests.filter(r => r.method === 'POST' && r.path === '/api/v1/sandboxes').length, 1);
  assert.deepEqual(s.grants.get('read_operator').capabilities, ['read_all_sandboxes', 'configure_platform']);
  assert.equal(s.db.attempts, 1); assert.equal(s.db.releases, 1);
});

test('accepted creation with failed attribution retains exact resource custody for cleanup', async () => {
  const s = scenario(); s.ctx.row.case_id = 'C275:ADM-010'; s.ctx.failAttribution = true;
  await assert.rejects(prepareBasicUsage(s.ctx, s.primitives), /request_attribution_missing/);
  for (const {fn} of s.deferred.reverse()) await fn();
  assert.ok(s.events.some(e => e[0] === 'document' && e[1] === 'sandbox-1'));
  assert.ok(s.events.some(e => e[0] === 'retire' && e[1] === 'sandbox-1'));
  assert.equal(s.db.releases, 1);
});

test('only the same invocation may borrow a live lease and borrowing exposes no release', async () => {
  const s = scenario(); const lease = await reserve(s.ctx, s.primitives, 'borrow-test');
  assert.throws(() => borrowCustody({...s.ctx}, lease), /active_parent_lease_required/);
  const borrowed = borrowCustody(s.ctx, lease); assert.equal(borrowed.release, undefined);
  await lease.release();
  assert.throws(() => borrowCustody(s.ctx, lease), /active_parent_lease_required/);
  assert.throws(() => borrowed.retainDocument('late'), /parent_lease_released/);
});
