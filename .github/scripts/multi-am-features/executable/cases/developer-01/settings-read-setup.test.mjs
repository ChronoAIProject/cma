import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {createAdminFixtures} from './fixture-provider.mjs';

const defaults = JSON.parse(readFileSync(new URL('./reset-contract.json', import.meta.url))).defaults;
const image = 'localhost/api@sha256:' + '1'.repeat(64);
function fixture({available = false, retained = true, denied = 403, actors} = {}) {
  const calls = [], cleanups = [], settings = structuredClone(defaults);
  if (retained) settings.model_budget.default_policy = {
    owner_day_limit: 1000, turn_limit: 100, max_calls_per_turn: 10, day_timezone: 'Asia/Singapore',
    input_token_units: 1, output_token_units: 1, max_unobserved_units: 10, usage_timeout_seconds: 30,
  };
  const body = {revision: 1, settings, model_budget_writes_available: available};
  const config = {scope: 'LOCAL_DISPOSABLE_TWO_AM', database_disposable: true,
    restart_authority: 'PARENT_ONLY', expected_api_source: '2'.repeat(40), expected_api_image: image,
    am_origins: {a: 'http://127.0.0.1:19001', b: 'http://127.0.0.1:19002'},
    actors: actors ?? {admin: {subject: 'fixture-admin'}, settings_operator: {subject: 'fixture-settings'},
      read_operator: {subject: 'fixture-read'}}};
  const ctx = {row: {case_id: 'C275:ADM-040'}, phase: 'steady', iteration: 1, am: 'a',
    defer: (_label, callback) => cleanups.push(callback), observe: async (_label, callback) => callback(),
    request: async request => {
      calls.push(request); assert.equal(request.method, 'GET');
      return request.actor === 'read_operator' ? {status: denied, body: {code: 'forbidden'}}
        : {status: 200, body: structuredClone(body)};
    }};
  const provider = createAdminFixtures({config, withExclusiveSettings: callback => callback(),
    adapter: {topology: async () => ({shared_database: true, coordination_enabled: true,
      ams: {a: {ready: true, image, pod_uid: 'a', instance_id: 'a'},
        b: {ready: true, image, pod_uid: 'b', instance_id: 'b'}}})}});
  return {calls, ctx, provider, body,
    cleanup: async () => {for (const callback of cleanups.toReversed()) await callback();}};
}

test('settings-read setup verifies real role and writer state using only public reads', async () => {
  const f = fixture();
  try {
    const setup = await f.provider.take('settings_read', f.ctx);
    assert.equal(setup.exclusiveSettings, true);
    assert.equal(typeof setup.faults.failNextRead, 'function');
    await assert.rejects(setup.faults.failNextRead({session: {}, path: '/api/v1/admin/settings'}),
      /admin_read_fault_browser_binding_missing/);
    assert.deepEqual(f.calls.map(call => call.actor), ['admin', 'settings_operator', 'read_operator']);
    assert.ok(f.calls.every(call => call.method === 'GET' && call.path === '/api/v1/admin/settings'));
  } finally {await f.cleanup();}
});

test('an absent optional availability flag remains explicitly unavailable, while malformed flags fail', async () => {
  const f = fixture(); delete f.body.model_budget_writes_available;
  try {await f.provider.take('settings_read', f.ctx);}
  finally {await f.cleanup();}
  const malformed = fixture({available: 'false'});
  try {await assert.rejects(malformed.provider.take('settings_read', malformed.ctx), /admin_read_settings_shape/);}
  finally {await malformed.cleanup();}
});

test('writable or empty spend configuration cannot pretend to exercise an unavailable retained policy', async () => {
  for (const [options, reason] of [[{available: true}, /admin_read_requires_unavailable_writer/],
    [{retained: false}, /admin_read_requires_retained_policy/]]) {
    const f = fixture(options);
    try {await assert.rejects(f.provider.take('settings_read', f.ctx), reason);}
    finally {await f.cleanup();}
  }
});

test('missing or reused actor identities block setup before any API request', async () => {
  for (const actors of [{}, {admin: {subject: 'same'}, settings_operator: {subject: 'same'},
    read_operator: {subject: 'different'}}]) {
    const f = fixture({actors});
    try {
      await assert.rejects(f.provider.take('settings_read', f.ctx), /admin_read_exact_roles_required/);
      assert.equal(f.calls.length, 0);
    } finally {await f.cleanup();}
  }
});

test('a read operator who can configure settings is refused as an overprivileged fixture', async () => {
  const f = fixture({denied: 200});
  try {await assert.rejects(f.provider.take('settings_read', f.ctx), /admin_read_permission_fixture/);}
  finally {await f.cleanup();}
});
