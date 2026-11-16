// Offline execution of the real case definition; these fixtures do NOT claim a product run.
import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { readFileSync } from 'node:fs';
import { cases } from './index.mjs';
import { executeCase } from '../../runner.mjs';
const definition = cases.find((c) => c.id === 'C275:cma-user.sandbox.always-on-off');
const row = JSON.parse(readFileSync(new URL('../../catalog.json', import.meta.url))).rows.find(
  (r) => r.case_id === definition.id,
);
const source = '2'.repeat(40),
  image = 'local/api@sha256:' + '3'.repeat(64);
function model({ stale = false, dropWrite = false, unsafeFixture = false } = {}) {
  const calls = [],
    events = [],
    requests = [];
  let state = { id: 'sbx-local', name: 'Disposable', status: 'active', always_on: true, revision: 7 };
  const path = '/api/v1/sandboxes/sbx-local',
    page = new EventEmitter();
  let selected = 'a';
  const response = (body) => ({
    url: () => 'http://local.invalid/api/agent-manager/v1/sandboxes/sbx-local/always-on',
    status: () => 200,
    request: () => ({ method: () => 'POST', postData: () => JSON.stringify(body), postDataJSON: () => body }),
    headers: () => ({ 'x-request-id': 'ui-write' }),
    json: async () => structuredClone(state),
  });
  const locator = (role, name) => ({
    waitFor: async () => {},
    isVisible: async () => true,
    innerText: async () =>
      role === 'heading'
        ? 'Disposable'
        : state.always_on && !calls.includes('write')
          ? 'An administrator approved Always on'
          : 'Always on is off',
    count: async () => (name instanceof RegExp ? 0 : 1),
    click: async () => {
      assert.equal(name, 'Turn off Always on');
      calls.push('write');
      const body = { always_on: false, expected_revision: state.revision - (stale ? 1 : 0) };
      if (!dropWrite) state = { ...state, always_on: false, revision: state.revision + 1 };
      requests.push({
        am: selected,
        serving_am: selected,
        method: 'POST',
        status: 200,
        request_id: 'ui-write',
        elapsed_ms: 1,
      });
      page.emit('response', response(body));
    },
  });
  page.getByRole = (role, { name } = {}) => locator(role, name);
  page.locator = () => locator('main');
  page.goto = async () => {
    calls.push('navigate');
    requests.push({
      am: selected,
      serving_am: selected,
      method: 'GET',
      status: 200,
      request_id: 'navigation',
      elapsed_ms: 1,
    });
  };
  page.reload = async () => {
    calls.push('reload');
    requests.push({
      am: selected,
      serving_am: selected,
      method: 'GET',
      status: 200,
      request_id: 'reload',
      elapsed_ms: 1,
    });
  };
  const fixture = {
    local: !unsafeFixture,
    run_owned: true,
    sandboxId: 'sbx-local',
    sandboxName: 'Disposable',
    cleanup: async () => {
      calls.push('cleanup');
      return { complete: true, owned_only: true };
    },
  };
  const environment = {
    source,
    image,
    phase: 'before_restart',
    iteration: 1,
    am: 'a',
    record: async (e) => events.push(e),
    fixtures: {
      'user-fixtures': { webSource: source, take: async () => fixture },
      browser: {
        open: async () => ({
          page,
          origin: 'http://local.invalid',
          switchAM: async (am) => {
            selected = am;
            calls.push('switch-' + am);
          },
          close: async () => calls.push('close'),
        }),
        evidence: async () => ({
          schema: 'multi-am-browser-evidence-v1',
          real_ui: true,
          compiled_source: source,
          compiled_manifest: 'sha256:' + '3'.repeat(64),
          secrets_redacted: true,
          requests,
        }),
      },
    },
    transport: {
      request: async (args) => {
        assert.equal(args.path, path);
        assert.equal(args.method, 'GET');
        calls.push('read-' + args.am);
        return {
          status: 200,
          body: structuredClone(state),
          timing: { request_id: 'peer-read', am: args.am, body_ms: 1 },
        };
      },
    },
  };
  return { environment, calls, events };
}
test('Always-on case writes once, reloads the peer, then joins durable false state', async () => {
  const { environment, calls } = model();
  const r = await executeCase(definition, row, environment);
  assert.equal(r.status, 'PASS');
  assert.equal(calls.filter((c) => c === 'write').length, 1);
  assert.equal(calls.includes('switch-b'), true);
  assert.equal(calls.filter((c) => c === 'read-b').length, 2);
  assert.equal(r.steps.length, row.required_steps.length);
  assert.deepEqual(calls.slice(-2), ['close', 'cleanup']);
});
test('stale revision cannot pass even if a faulty backend accepts the write; cleanup and evidence retained', async () => {
  const { environment, calls, events } = model({ stale: true });
  const r = await executeCase(definition, row, environment);
  assert.equal(r.status, 'FAIL');
  assert.equal(r.primary_failure, 'always_on_expected_revision');
  assert.equal(calls.filter((c) => c === 'write').length, 1);
  assert.equal(calls.at(-1), 'cleanup');
  assert.equal(
    events.some((e) => e.kind === 'observation' && e.label === 'browser_finality'),
    true,
  );
});
test('nonlocal fixture refuses before browser navigation or mutation', async () => {
  const { environment, calls } = model({ unsafeFixture: true });
  const r = await executeCase(definition, row, environment);
  assert.equal(r.status, 'FAIL');
  assert.equal(r.primary_failure, 'local_owned_fixture_required');
  assert.deepEqual(calls, ['cleanup']);
});

test('optimistic UI and acknowledgement cannot hide an unchanged durable value', async () => {
  const { environment, calls } = model({ dropWrite: true });
  const result = await executeCase(definition, row, environment);
  assert.equal(result.status, 'FAIL');
  assert.equal(result.primary_failure, 'always_on_off_durable');
  assert.equal(calls.filter((c) => c === 'write').length, 1);
  assert.equal(calls.at(-1), 'cleanup');
});
