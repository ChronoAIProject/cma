import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {cases} from './index.mjs';
import {executeCase} from '../../runner.mjs';
import {settingsSession, setup} from './common.mjs';
import {validateAdmission} from './admission-read.mjs';

const catalog = JSON.parse(await readFile(new URL('../../catalog.json', import.meta.url)));
const environment = () => ({phase: 'before_restart', iteration: 1, am: 'a',
  source: '2'.repeat(40), image: 'localhost/api@sha256:' + '1'.repeat(64), fixtures: {}});

test('every exported ADMIN case has the exact canonical step identities', () => {
  assert.equal(new Set(cases.map(c => c.id)).size, cases.length);
  for (const c of cases) {
    const row = catalog.rows.find(r => r.case_id === c.id);
    assert.ok(row); assert.deepEqual(c.steps, row.required_steps);
    assert.ok(c.fixtures.includes('admin-fixtures'));
  }
});

test('no case can pass or dispatch when authentic fixture adapters are absent', async () => {
  for (const c of cases) {
    const result = await executeCase(c, catalog.rows.find(r => r.case_id === c.id), environment());
    assert.equal(result.status, 'BLOCKED', c.id); assert.equal(result.requests.length, 0);
  }
});

function stateContext() {
  let state = {revision: 3, settings: {retention: {agent_log_days: 30}, limits: {count: 5}}};
  const calls = [], cleanups = [];
  return {calls, cleanups, get state() { return structuredClone(state); },
    set state(value) {state = structuredClone(value);},
    ctx: {am: 'a', defer: (label, fn) => cleanups.push([label, fn]),
      request: async args => {
        calls.push(structuredClone(args));
        if (args.method !== 'GET') {
          assert.equal(args.body.expected_revision, state.revision);
          state = {revision: state.revision + 1, settings: args.method === 'PUT'
            ? args.body.settings : {...state.settings, retention: args.body.retention}};
        }
        return {status: 200, body: structuredClone(state)};
      }}};
}

test('settings restoration reconciles an accepted write with a lost acknowledgement once', async () => {
  const f = stateContext(), s = await settingsSession(f.ctx, {exclusiveSettings: true});
  const desired = {...s.original.settings, retention: {agent_log_days: 31}};
  s.own(desired); f.state = {revision: 4, settings: desired};
  await s.restore(); await s.restore();
  assert.deepEqual(f.state.settings, s.original.settings);
  const writes = f.calls.filter(c => c.method !== 'GET');
  assert.equal(writes.length, 1); assert.equal(writes[0].body.expected_revision, 4);
  assert.equal(writes[0].am, 'b');
});

test('settings cleanup refuses a foreign change rather than overwriting it', async () => {
  const f = stateContext(), s = await settingsSession(f.ctx, {exclusiveSettings: true});
  s.own({...s.original.settings, retention: {agent_log_days: 31}});
  f.state = {revision: 5, settings: {...s.original.settings, limits: {count: 99}}};
  await assert.rejects(s.restore(), /admin_restore_foreign_settings/);
  assert.equal(f.calls.filter(c => c.method !== 'GET').length, 0);
});

test('setup registers custody before refusing a nonlocal fixture', async () => {
  const cleanups = [];
  const ctx = {fixture: () => ({take: async () => ({scope: 'PRODUCTION', cleanup: async () => {}})}),
    defer: (name, fn) => cleanups.push([name, fn])};
  await assert.rejects(setup(ctx, 'settings'), /admin_fixture_not_local_disposable/);
  assert.equal(cleanups.length, 1);
});

test('admission history rejects gaps, missing records and broken stop transitions', () => {
  const stop = {since: 1, revision: 1, actor: 'synthetic', reason: 'test'};
  const one = {revision: 1, at: 1, actor: 'synthetic', reason: 'test', old_stop: null, new_stop: stop};
  const two = {revision: 2, at: 2, actor: 'synthetic', reason: 'test', old_stop: stop, new_stop: null};
  const valid = {revision: 2, writes_available: true, operator_stop: null, last_change: two, history: [two, one]};
  assert.equal(validateAdmission(valid), true);
  for (const corrupt of [
    {...valid, history: [two]},
    {...valid, history: [two, {...one, revision: 0}]},
    {...valid, history: [{...two, old_stop: {...stop, actor: 'other'}}, one]},
    {...valid, operator_stop: stop},
  ]) assert.equal(validateAdmission(corrupt), false);
});
