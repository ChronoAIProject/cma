import test from 'node:test';
import assert from 'node:assert/strict';
import {Blocked} from '../../runner.mjs';
import {prepareAdminRoles, prepareBasicUsage, roleCapabilities} from './setups.mjs';
import {expectedReport, intervalTotals} from './allocation-truth.mjs';

const actors = ['admin', 'user', 'outsider', ...Object.keys(roleCapabilities)];
function identities() {
  return Object.fromEntries(actors.map(actor => [actor, {actor, subject: 'subject-' + actor}]));
}
function context(caseId = 'C275:ADM-002', sessionCaps = roleCapabilities) {
  const deferred = [], state = new Map(Object.entries(roleCapabilities).map(([actor, capabilities]) =>
    [actor, {actor, subject: 'subject-' + actor, revision: 1, capabilities: [...capabilities]}]));
  const ctx = {
    row: {case_id: caseId}, am: 'a', deferred,
    defer(_label, operation) { deferred.push(operation); },
    async observe() {},
    async request({actor, am}) {
      const grant = state.get(actor);
      return {status: 200, body: {authenticated: true, subject: grant.subject,
        capabilities: [...(sessionCaps[actor] ?? grant.capabilities)]}, timing: {am, request_id: actor + '-' + am}};
    },
  };
  return {ctx, state, deferred};
}

function authFor(state) {
  const calls = [];
  return {calls,
    async readGrant({actor}) { calls.push(['read', actor]); return structuredClone(state.get(actor)); },
    async replaceGrant({actor, expected_revision, capabilities}) {
      calls.push(['replace', actor, expected_revision, [...capabilities]]);
      const current = state.get(actor); assert.equal(current.revision, expected_revision);
      const next = {...current, revision: current.revision + 1, capabilities: [...capabilities]}; state.set(actor, next);
      return structuredClone(next);
    }};
}

test('admin role setup blocks before auth writes when parent primitives are absent', async () => {
  const {ctx} = context();
  await assert.rejects(prepareAdminRoles(ctx, {identities: identities()}), error =>
    error instanceof Blocked && error.message === 'admin09_auth_grant_primitives_required');
});

test('admin role setup uses named grant CAS and restores the original grants', async () => {
  const {ctx, state, deferred} = context(); const auth = authFor(state);
  state.get('read_operator').capabilities.push('configure_platform');
  const fixture = await prepareAdminRoles(ctx, {auth, identities: identities()});
  assert.equal(fixture.scope, 'LOCAL_DISPOSABLE_TWO_AM');
  for (const [actor, capabilities] of Object.entries(roleCapabilities))
    assert.deepEqual(state.get(actor).capabilities, capabilities);
  assert.ok(auth.calls.some(call => call[0] === 'replace' && call[1] === 'read_operator'));
  await fixture.cleanup();
  assert.deepEqual([...state.get('read_operator').capabilities].sort(), ['configure_platform', 'read_all_sandboxes']);
  // Idempotent cleanup also runs safely through the parent defer stack.
  for (const operation of deferred.reverse()) await operation();
  assert.deepEqual([...state.get('read_operator').capabilities].sort(), ['configure_platform', 'read_all_sandboxes']);
});

test('a session capability mismatch is a failure and auth cleanup remains registered', async () => {
  const {ctx, deferred, state} = context('C275:ADM-002', {...roleCapabilities, read_operator: []});
  const auth = authFor(state);
  await assert.rejects(prepareAdminRoles(ctx, {auth, identities: identities()}), /admin09_auth_session_mismatch/);
  assert.ok(deferred.length > 0);
  for (const operation of deferred.reverse()) await operation();
  assert.deepEqual(state.get('read_operator').capabilities, roleCapabilities.read_operator);
});

test('advanced usage setup blocks before a disposable reservation', async () => {
  let reserved = false;
  const ctx = {row: {case_id: 'C275:ADM-016'}, defer() {}};
  await assert.rejects(prepareBasicUsage(ctx, {disposableDatabase: {reserve() { reserved = true; }}}),
    error => error instanceof Blocked && error.message === 'admin09_advanced_usage_recipe_missing');
  assert.equal(reserved, false);
});

test('allocation truth uses half-open interval overlap and independent expected totals', () => {
  const record = {owner: 'subject-user', sandbox_id: 'sandbox-1', tracking_started_at: 0, events: [
    {revision: 1, started_at: 0, ended_at: 10,
      allocation: {cpu_millis: 1000, memory_mib: 1024, disk_gib: 10, running: true},
      next_allocation: {cpu_millis: 2000, memory_mib: 2048, disk_gib: 20, running: false}},
  ]};
  const totals = intervalTotals(record, 5, 15);
  assert.equal(totals.uptime_seconds, 5);
  assert.equal(totals.cpu_core_hours, (1000 * 5 + 2000 * 5) / 3600000);
  const expected = expectedReport([record], {from: 0, to: 15, filters: {}});
  assert.equal(expected.sandbox_count, 1);
  assert.equal(expected.allocated_period.disk_gib_hours, (10 * 10 + 20 * 5) / 3600);
});

