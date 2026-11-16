import test from 'node:test';
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {readFileSync} from 'node:fs';
import {resetDefaults, settingsPatchSetup} from './settings-setup.mjs';
import {publishedProfile} from './settings-profile.mjs';
import {settingsPatch} from './settings-api.mjs';
import {createAdminFixtures} from './fixture-provider.mjs';
import {executeCase} from '../../runner.mjs';

const source = execFileSync('git', ['rev-parse', 'HEAD'], {encoding: 'utf8'}).trim();
const defaults = JSON.parse(readFileSync(new URL('./reset-contract.json', import.meta.url))).defaults;
const image = 'localhost/api@sha256:' + '1'.repeat(64);
const config = {scope: 'LOCAL_DISPOSABLE_TWO_AM', database_disposable: true,
  restart_authority: 'PARENT_ONLY', expected_api_source: source, expected_api_image: image,
  am_origins: {a: 'http://127.0.0.1:19001', b: 'http://127.0.0.1:19002'}};

// Offline HTTP contract only. This deliberately cannot stand in for a parent
// two-AM run; it catches sequencing, lost-response custody and cleanup mistakes.
function fixture({unknownCreate = false, dropResetAudit = false} = {}) {
  let revision = 7, state = structuredClone(defaults), profile, count = 0;
  state.retention.agent_log_days = 25;
  const original = structuredClone(state), calls = [], audit = [], cleanups = [];
  const snapshot = () => ({revision, settings: structuredClone(state), model_budget_writes_available: true});
  const request = async args => {
    calls.push(structuredClone(args));
    const {method, path, body, actor} = args;
    let result, status = 200;
    if (actor === 'user') status = 403;
    else if (path.endsWith('/settings') && method === 'GET') result = snapshot();
    else if (path.includes('/activity?')) result = {items: structuredClone(audit)};
    else if (path.endsWith('/settings') || path.endsWith('/settings/reset')) {
      if (body.expected_revision !== revision) status = 409;
      else if (method === 'PATCH' && (body.unknown_section || Object.keys(body).length === 1)) status = 422;
      else {
        if (method === 'POST') state = structuredClone(defaults);
        else if (method === 'PUT') state = structuredClone(body.settings);
        else state = {...state, ...Object.fromEntries(Object.entries(body).filter(([k]) => k !== 'expected_revision'))};
        revision++;
        if (!(dropResetAudit && method === 'POST')) audit.push({target: {id: 'platform_settings'},
          detail: {revision: String(revision)}, outcome: {status: 'succeeded'}});
        result = snapshot();
      }
    } else if (path === '/api/v1/agent-profiles' && method === 'POST') {
      profile = {...body, id: 'agp_test', draft_version: 1}; status = 201;
      if (unknownCreate) throw Error('lost_create_ack');
      result = structuredClone(profile);
    } else if (path.endsWith('/revisions') && method === 'POST') {
      status = 201; result = {profile_id: profile.id, revision: 1, digest: 'sha256:fixture'};
    } else if (path.endsWith('/revisions/1')) {
      result = {profile_id: profile.id, revision: 1, digest: 'sha256:fixture'};
    } else if (path.endsWith('/retire') && method === 'POST') {
      assert.notEqual(state.defaults.agent_profile.id, profile.id);
      profile.retired_at = new Date().toISOString(); result = structuredClone(profile);
    } else if (path === '/api/v1/agent-profiles/agp_test') result = structuredClone(profile);
    else throw Error('unexpected_fixture_route');
    return {status, body: result, timing: {am: args.am, request_id: 'offline-' + ++count}};
  };
  return {original, calls, snapshot, request, cleanups, get profile() {return profile;},
    ctx: {row: {case_id: 'C275:ADM-051'}, am: 'a', request,
      defer: (_name, fn) => cleanups.push(fn), observe: async (_label, op) => op()}};
}

test('reset defaults are bound to exact immutable product source, unknown source is blocked', async () => {
  assert.deepEqual(await resetDefaults(source), defaults);
  await assert.rejects(resetDefaults('0'.repeat(40)), /admin_reset_source_unavailable/);
  await assert.rejects(resetDefaults('HEAD'), /admin_reset_source_unbound/);
});

test('public patch recipe prepares eight changed sections and a published readable profile', async () => {
  const f = fixture();
  const setup = await settingsPatchSetup(f.ctx, {config});
  assert.equal(setup.patchSections.length, 8);
  assert.ok(setup.patchSections.every(({section, value}) =>
    JSON.stringify(value) !== JSON.stringify(f.original[section])));
  assert.deepEqual(setup.resetDefaults, defaults);
  assert.deepEqual(setup.patchSections.find(s => s.section === 'defaults').value.agent_profile,
    {id: 'agp_test', revision: 1});
  await setup.cleanup(); await setup.cleanup();
  assert.ok(f.profile.retired_at);
  assert.equal(f.calls.filter(r => r.path.endsWith('/retire')).length, 1);
  assert.equal(f.calls.filter(r => r.method !== 'GET' && r.path.includes('/settings')).length, 0);
});

test('unknown profile create is never replayed and prevents false cleanup success', async () => {
  const f = fixture({unknownCreate: true});
  await assert.rejects(publishedProfile(f.ctx), /lost_create_ack/);
  await assert.rejects(f.cleanups[0](), /admin_profile_unknown_create_requires_parent_cleanup/);
  assert.equal(f.calls.length, 1);
});

test('a lost retirement acknowledgement cannot cause a second retirement dispatch', async () => {
  const f = fixture(), original = f.ctx.request;
  let retirementAttempts = 0;
  f.ctx.request = async args => {
    if (args.path.endsWith('/retire')) {retirementAttempts++; throw Error('lost_retire_ack');}
    return original(args);
  };
  const profile = await publishedProfile(f.ctx);
  await assert.rejects(profile.cleanup(), /lost_retire_ack/);
  await assert.rejects(profile.cleanup(), /admin_profile_retire_unknown_requires_parent_cleanup/);
  assert.equal(retirementAttempts, 1);
});

for (const dropResetAudit of [false, true]) test('patch case restores then retires on '
  + (dropResetAudit ? 'missing audit failure' : 'successful offline sequence'), async () => {
  const f = fixture({dropResetAudit}), lease = [];
  const adapter = {topology: async () => ({shared_database: true, coordination_enabled: true,
    ams: {a: {ready: true, image, instance_id: 'a', pod_uid: 'a'},
      b: {ready: true, image, instance_id: 'b', pod_uid: 'b'}}})};
  const provider = createAdminFixtures({config, adapter, withExclusiveSettings: async callback => {
    lease.push('acquired'); try {await callback();} finally {lease.push('released');}
  }});
  const result = await executeCase(settingsPatch, {case_id: settingsPatch.id, family: 'admin',
    required_steps: settingsPatch.steps}, {phase: 'before_restart', iteration: 1, am: 'a', source, image,
    fixtures: {'admin-fixtures': provider}, transport: {request: f.request}});
  assert.equal(result.status, dropResetAudit ? 'FAIL' : 'PASS', result.error_code);
  if (dropResetAudit) assert.equal(result.error_code, 'admin_reset_audit_missing');
  assert.deepEqual(f.snapshot().settings, f.original);
  assert.ok(f.profile.retired_at); assert.equal(result.requires_parent_cleanup, true);
  assert.ok(result.cleanup.every(c => c.status === 'PASS'));
  assert.deepEqual(lease, ['acquired', 'released']);
});
