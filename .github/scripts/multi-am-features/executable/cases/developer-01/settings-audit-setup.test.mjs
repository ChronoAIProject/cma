import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {settingsAuditSetup, matchesSettingsAudit} from './settings-audit-setup.mjs';
import {admissionAuditCohort} from './admission-audit.mjs';
import {createAdminFixtures} from './fixture-provider.mjs';
import {executeCase} from '../../runner.mjs';
import {setup as takeSetup} from './common.mjs';

const image = 'localhost/api@sha256:' + '1'.repeat(64);
const config = {scope: 'LOCAL_DISPOSABLE_TWO_AM', database_disposable: true,
  restart_authority: 'PARENT_ONLY', expected_api_source: '2'.repeat(40), expected_api_image: image,
  am_origins: {a: 'http://127.0.0.1:19001', b: 'http://127.0.0.1:19002'},
  actors: {admin: {subject: 'synthetic-admin'}, settings_operator: {subject: 'synthetic-settings'}}};
const topology = () => ({shared_database: true, coordination_enabled: true,
  ams: {a: {ready: true, image, instance_id: 'a', pod_uid: 'a'},
    b: {ready: true, image, instance_id: 'b', pod_uid: 'b'}}});

// Offline contract model. No browser, Mongo, AM or feature acceptance is run.
// Real execution must retain the parent image, ingress and disposal receipts.
function fixture(options = {}) {
  const defaults = JSON.parse(readFileSync(new URL('./reset-contract.json', import.meta.url))).defaults;
  let settings = {revision: 7, settings: structuredClone(defaults), model_budget_writes_available: true};
  let control = {revision: 0, operator_stop: null, last_change: null, writes_available: true};
  const original = structuredClone(settings), calls = [], audit = [], history = [], cleanups = [], events = [];
  const anchor = Math.floor(Date.now() / 1000) - 5;
  const snapshot = () => structuredClone(settings);
  const admission = () => ({...structuredClone(control), history: structuredClone(history.slice(0, 20))});
  const addAudit = (target, revision, at, status) => {
    audit.unshift({id: 'audit-' + (audit.length + 1), actor: 'synthetic-admin', action: 'configure',
      at: new Date(at * 1000).toISOString(), target: {kind: 'resource', id: target},
      outcome: {status: 'succeeded'}, detail: {revision: String(revision), ...(status ? {status} : {})}});
  };
  const request = async args => {
    calls.push(structuredClone(args));
    const {method, path, body, actor} = args;
    let result, status = 200;
    if (path.startsWith('/api/v1/admin/activity')) {
      if (actor === 'settings_operator' && !options.overprivileged) status = 403;
      else {
        let items = structuredClone(audit.slice(0, 50));
        if (control.revision >= 50) {
          if (options.dropAudit) items.pop();
          if (options.duplicateAudit) items[1] = structuredClone(items[0]);
          if (options.wrongActor) items[0].actor = 'another-admin';
          if (options.wrongTarget) items[0].target.id = 'platform_settings';
          if (options.oldTimestamp) items[0].at = new Date(anchor * 1000).toISOString();
        }
        result = {items, next: audit.length > 50 ? 'cursor' : null};
      }
    } else if (path.endsWith('/settings')) {
      if (method !== 'GET') {
        assert.equal(body.expected_revision, settings.revision);
        settings = {...settings, revision: settings.revision + 1, settings: method === 'PUT'
          ? structuredClone(body.settings) : {...settings.settings, retention: body.retention}};
        addAudit('platform_settings', settings.revision, anchor);
        if (options.lostPatch && method === 'PATCH') throw Error('lost_settings_ack');
      }
      result = snapshot();
    } else if (path.endsWith('/admission')) result = admission();
    else if (/\/admission\/(stop|resume)$/.test(path)) {
      const stop = path.endsWith('/stop');
      assert.equal(body.expected_revision, control.revision);
      if (!stop && options.lostResumeBeforeCommit) throw Error('lost_resume_ack');
      if (stop && options.lostStopBeforeCommit) throw Error('lost_stop_ack');
      const revision = control.revision + 1, at = anchor + 1;
      const next = stop ? {since: at, revision, actor: 'synthetic-admin', reason: body.reason} : null;
      const change = {revision, at, actor: 'synthetic-admin', reason: body.reason,
        old_stop: control.operator_stop, new_stop: next};
      control = {revision, operator_stop: next, last_change: change, writes_available: true};
      history.unshift(structuredClone(change));
      addAudit('admission_control', revision, at, stop ? 'stopped' : 'open');
      if (stop && options.lostStop) throw Error('lost_stop_ack');
      if (!stop && options.lostResume) throw Error('lost_resume_ack');
      result = structuredClone(control);
    } else throw Error('unexpected_offline_route');
    assert.ok((args.allowed ?? [200]).includes(status), 'unexpected_http_' + status);
    return {status, body: result, timing: {am: args.am, request_id: 'offline-' + calls.length}};
  };
  return {calls, cleanups, events, original, snapshot, admission, audit,
    setControl(value) {control = value; history.splice(0, history.length, value.last_change);},
    ctx: {row: {case_id: 'C275:ADM-052'}, phase: 'before_restart', iteration: 1, am: 'a', request,
      defer: (label, callback) => cleanups.push({label, callback}),
      observe: async (label, callback) => {const value = await callback(); events.push({label, value}); return value;}},
    async unwind() {
      const failures = [];
      for (const {callback} of cleanups.toReversed()) {
        try {await callback();} catch (error) {failures.push(error);}
      }
      return failures;
    }};
}

test('the audit recipe creates 50 real admission transitions without changing its settings revision', async () => {
  const f = fixture();
  let held = false;
  const provider = createAdminFixtures({config, adapter: {topology: async () => topology()},
    withExclusiveSettings: async callback => {
      held = true; try {await callback();} finally {held = false;}
    }});
  const setup = await provider.take('settings_audit', f.ctx);
  assert.equal(held, true);
  const changed = f.snapshot(), match = f.audit[0];
  assert.equal(setup.actor, config.actors.admin.subject);
  assert.ok(matchesSettingsAudit(match, changed.revision));
  assert.notDeepEqual(changed.settings, f.original.settings);
  await setup.useAuditCohort('no_current_revision');
  assert.deepEqual(f.snapshot(), changed);
  assert.equal(f.admission().revision, 50); assert.equal(f.admission().operator_stop, null);
  assert.equal(f.audit.slice(0, 50).some(row => matchesSettingsAudit(row, changed.revision)), false);
  assert.equal(f.audit[50].id, match.id);
  assert.equal(f.calls.filter(r => r.path.endsWith('/stop')).length, 25);
  assert.equal(f.calls.filter(r => r.path.endsWith('/resume')).length, 25);
  assert.ok(f.calls.filter(r => r.path.endsWith('/resume')).every(r => r.am === 'b'));
  await assert.rejects(setup.useAuditCohort('no_current_revision'), /admin_audit_cohort_replayed/);
  await setup.cleanup(); await setup.cleanup();
  assert.equal(held, false); assert.deepEqual(f.snapshot().settings, f.original.settings);
  assert.equal(f.calls.filter(r => r.method === 'PUT').length, 1);
  assert.equal(f.events[0].value.final_database_cleanup, 'PARENT_MUST_CONFIRM_BEFORE_CAMPAIGN_PASS');
  assert.deepEqual(await f.unwind(), []);
});

test('missing or overprivileged named actors block the recipe before any mutation', async () => {
  for (const actors of [{}, {admin: {subject: 'same'}, settings_operator: {subject: 'same'}}]) {
    const f = fixture();
    await assert.rejects(settingsAuditSetup(f.ctx, {config: {...config, actors}}), /admin_audit_exact_roles_required/);
    assert.equal(f.calls.length, 0);
  }
  const f = fixture({overprivileged: true});
  await assert.rejects(settingsAuditSetup(f.ctx, {config}), /unexpected_http_200/);
  assert.ok(f.calls.every(call => call.method === 'GET'));
});

for (const option of ['dropAudit', 'duplicateAudit', 'wrongActor', 'wrongTarget', 'oldTimestamp']) {
  test('the real audit page is mandatory: reject ' + option + ' and restore settings', async () => {
    const f = fixture({[option]: true}), setup = await settingsAuditSetup(f.ctx, {config});
    await assert.rejects(setup.useAuditCohort('no_current_revision'), /admin_audit_displacement_/);
    await setup.cleanup();
    assert.deepEqual(f.snapshot().settings, f.original.settings);
    assert.equal(f.admission().operator_stop, null);
  });
}

for (const [option, stops, resumes] of [['lostStopBeforeCommit', 1, 0], ['lostStop', 1, 1], ['lostResume', 1, 1]]) {
  test(option + ' is reconciled by reads and never replays the unknown mutation', async () => {
    const f = fixture({[option]: true}), setup = await settingsAuditSetup(f.ctx, {config});
    await assert.rejects(setup.useAuditCohort('no_current_revision'), /lost_.*_ack/);
    await setup.cleanup(); await setup.cleanup();
    assert.equal(f.calls.filter(r => r.path.endsWith('/stop')).length, stops);
    assert.equal(f.calls.filter(r => r.path.endsWith('/resume')).length, resumes);
    assert.equal(f.admission().operator_stop, null);
    assert.deepEqual(f.snapshot().settings, f.original.settings);
  });
}

test('an unconfirmed resume stays a cleanup failure without replay, while settings are restored', async () => {
  const f = fixture({lostResumeBeforeCommit: true}), setup = await settingsAuditSetup(f.ctx, {config});
  await assert.rejects(setup.useAuditCohort('no_current_revision'), /lost_resume_ack/);
  for (let i = 0; i < 2; i++) await assert.rejects(setup.cleanup(), error =>
    error.errors.some(e => e.message === 'admin_audit_unknown_resume_requires_parent_cleanup'));
  assert.equal(f.calls.filter(r => r.path.endsWith('/resume')).length, 1);
  assert.deepEqual(f.snapshot().settings, f.original.settings);
  assert.notEqual(f.admission().operator_stop, null);
});

test('a foreign stop is never resumed by fixture cleanup', async () => {
  const f = fixture({lostStop: true}), setup = await settingsAuditSetup(f.ctx, {config});
  await assert.rejects(setup.useAuditCohort('no_current_revision'), /lost_stop_ack/);
  const current = f.admission();
  current.operator_stop.actor = 'foreign'; current.operator_stop.reason = 'foreign change';
  current.last_change.actor = 'foreign'; current.last_change.reason = 'foreign change';
  current.last_change.new_stop = current.operator_stop; f.setControl(current);
  await assert.rejects(setup.cleanup(), error => error.errors.some(e => e.message === 'admin_audit_admission_foreign_change'));
  assert.equal(f.calls.filter(r => r.path.endsWith('/resume')).length, 0);
  assert.deepEqual(f.snapshot().settings, f.original.settings);
});

test('a lost settings setup acknowledgement still restores the accepted settings through registered custody', async () => {
  const f = fixture({lostPatch: true});
  await assert.rejects(settingsAuditSetup(f.ctx, {config}), /lost_settings_ack/);
  assert.deepEqual(await f.unwind(), []);
  assert.deepEqual(f.snapshot().settings, f.original.settings);
  assert.equal(f.calls.filter(r => r.method === 'PATCH').length, 1);
  assert.equal(f.calls.filter(r => r.method === 'PUT').length, 1);
});

test('future clock, invalid cohort and changed admission fail without cohort writes', async () => {
  const f = fixture(), cohort = await admissionAuditCohort(f.ctx, config.actors.admin.subject);
  await assert.rejects(cohort.displace(new Date(Date.now() + 10000).toISOString()), /admin_audit_server_clock_unbound/);
  assert.ok(f.calls.every(r => r.method === 'GET'));
  await cohort.cleanup();
  const g = fixture(), setup = await settingsAuditSetup(g.ctx, {config});
  await assert.rejects(setup.useAuditCohort('invented'), /admin_audit_unknown_cohort/);
  await setup.cleanup();
  const h = fixture();
  h.setControl({revision: 0, operator_stop: null, last_change: null, writes_available: false});
  await assert.rejects(admissionAuditCohort(h.ctx, config.actors.admin.subject), /admin_audit_admission_open_writer_required/);
});

test('settings audit matches exact action, target kind, revision and successful outcome', () => {
  const row = {action: 'configure', target: {kind: 'resource', id: 'platform_settings'},
    outcome: {status: 'succeeded'}, detail: {revision: '8'}};
  assert.equal(matchesSettingsAudit(row, 8), true);
  for (const mutate of [r => r.action = 'read', r => r.target.kind = 'subject',
    r => r.target.id = 'admission_control', r => r.detail.revision = '9', r => r.outcome.status = 'failed']) {
    const wrong = structuredClone(row); mutate(wrong); assert.equal(matchesSettingsAudit(wrong, 8), false);
  }
});

test('runner retains the primary failure and unconfirmed resume cleanup failure under the parent lease', async () => {
  const f = fixture({lostResumeBeforeCommit: true});
  let held = false;
  const provider = createAdminFixtures({config, adapter: {topology: async () => topology()},
    withExclusiveSettings: async callback => {
      held = true; try {await callback();} finally {held = false;}
    }});
  const definition = {id: 'C275:ADM-052', scope: 'global', fixtures: ['admin-fixtures'],
    steps: ['step_1'], async run(ctx) {
      const setup = await takeSetup(ctx, 'settings_audit');
      await ctx.step('step_1', async () => {await setup.useAuditCohort('no_current_revision');});
    }};
  const result = await executeCase(definition, {case_id: definition.id, family: 'admin',
    required_steps: definition.steps}, {phase: 'before_restart', iteration: 1, am: 'a', image,
    source: config.expected_api_source, fixtures: {'admin-fixtures': provider}, transport: {
      request: args => {assert.equal(held, true); return f.ctx.request(args);},
    }});
  assert.equal(result.status, 'FAIL'); assert.equal(result.primary_failure, 'lost_resume_ack');
  assert.ok(result.cleanup.some(row => row.error_code === 'admin_audit_unknown_resume_requires_parent_cleanup'));
  assert.equal(result.requires_parent_cleanup, true);
  assert.equal(f.calls.filter(r => r.path.endsWith('/resume')).length, 1);
  assert.deepEqual(f.snapshot().settings, f.original.settings);
  assert.equal(held, false);
});
