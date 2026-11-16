import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { readFileSync } from 'node:fs';
import { cases } from './index.mjs';
import { executeCase, Blocked } from '../../runner.mjs';
import { loadRegistry } from '../../registry.mjs';
import { setup, finish, capture, hash, canonicalPath } from './helpers.mjs';
const catalog = JSON.parse(readFileSync(new URL('../../catalog.json', import.meta.url)));
const assignment = JSON.parse(readFileSync(new URL('../../assignments/developer-06.json', import.meta.url)));
const source = '2'.repeat(40),
  image = 'local/api@sha256:' + '3'.repeat(64);
const env = () => ({ phase: 'before_restart', iteration: 1, am: 'a', image, source, fixtures: {} });
const row = {
  case_id: 'contract',
  family: 'user',
  required_steps: ['step_1', 'expected_result'],
  target_seconds: null,
};
const definition = (run) => ({
  id: 'contract',
  scope: 'browser',
  fixtures: ['browser', 'user-fixtures'],
  steps: row.required_steps,
  run,
});
function fixtureEnvironment({ browserFails = false, cleanupComplete = true, evidencePatch = {} } = {}) {
  const e = env(),
    calls = [];
  const page = new EventEmitter();
  const evidence = {
    real_ui: true,
    compiled_source: source,
    secrets_redacted: true,
    requests: [
      {
        request_id: 'request-one',
        am: 'b',
        elapsed_ms: 5,
        agent_id_sha256: hash('agent-one'),
        owner: 'a',
        owner_epoch: 1,
        owner_stable: true,
      },
    ],
    ...evidencePatch,
  };
  e.fixtures = {
    'user-fixtures': {
      webSource: source,
      take: async () => ({
        local: true,
        run_owned: true,
        agentId: 'agent-one',
        cleanup: async () => {
          calls.push('fixture');
          return { complete: cleanupComplete, owned_only: true };
        },
      }),
    },
    browser: {
      open: async () => {
        if (browserFails) throw Error('browser_open_failed');
        return { page, origin: 'http://local.invalid', close: async () => calls.push('browser') };
      },
      evidence: async (selected) => {
        assert.equal(selected, page);
        return evidence;
      },
    },
  };
  return { e, calls };
}
async function boundaryCase(ctx) {
  await ctx.step('step_1', async () => {
    await setup(ctx, 'contract');
    ctx.assert(true, 'setup_done');
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(true, 'case_observed');
    await finish(ctx);
  });
}

test('all 34 retained IDs and exact original steps are registered, with no transferred rows', async () => {
  const registered = await loadRegistry();
  assert.equal(cases.length, 34);
  assert.equal([...registered.cases.values()].filter(value => value.owner === 'developer-06').length, 34);
  assert.deepEqual(cases.map((c) => c.id).sort(), [...assignment.case_ids].sort());
  for (const c of cases) {
    const original = catalog.rows.find((r) => r.case_id === c.id);
    assert.deepEqual(c.steps, original.required_steps);
    assert.equal(c.fixtures.includes('browser'), true);
    assert.equal(c.fixtures.includes('user-fixtures'), true);
  }
  assert.equal(
    cases.some((c) => /usage\.|triggers\.draft|navigation\./.test(c.id)),
    false,
  );
});
test('every case is BLOCKED before any request when authentic fixtures are absent', async () => {
  for (const c of cases) {
    let invoked = 0;
    const e = env();
    e.transport = {
      request: async () => {
        invoked++;
        throw Error('unexpected_request');
      },
    };
    const result = await executeCase(
      c,
      catalog.rows.find((r) => r.case_id === c.id),
      e,
    );
    assert.equal(result.status, 'BLOCKED', c.id);
    assert.equal(invoked, 0);
    assert.equal(result.steps.length, 0);
  }
});
test('named fixture refusal is BLOCKED and never turns into a feature PASS', async () => {
  const { e } = fixtureEnvironment();
  e.fixtures['user-fixtures'].take = async () => {
    throw new Blocked('fixture_scenario_unavailable');
  };
  const result = await executeCase(definition(boundaryCase), row, e);
  assert.equal(result.status, 'BLOCKED');
  assert.equal(result.error_code, 'fixture_scenario_unavailable');
});
test('fixture custody is cleaned if browser acquisition fails', async () => {
  const { e, calls } = fixtureEnvironment({ browserFails: true });
  const r = await executeCase(definition(boundaryCase), row, e);
  assert.equal(r.status, 'FAIL');
  assert.equal(r.primary_failure, 'browser_open_failed');
  assert.deepEqual(calls, ['fixture']);
});
test('incomplete cleanup prevents PASS and closes browser before owned resources', async () => {
  const { e, calls } = fixtureEnvironment({ cleanupComplete: false });
  const r = await executeCase(definition(boundaryCase), row, e);
  assert.equal(r.status, 'FAIL');
  assert.equal(
    r.cleanup.find((x) => x.label === 'dispose_owned_fixture').error_code,
    'fixture_cleanup_incomplete',
  );
  assert.deepEqual(calls, ['browser', 'fixture']);
});
test('browser evidence refuses mocked UI, stale image, missing request ID and owner-side execution', async () => {
  for (const patch of [
    { real_ui: false },
    { compiled_source: '4'.repeat(40) },
    { secrets_redacted: false },
    { requests: [] },
    { requests: [{ request_id: '', am: 'b', elapsed_ms: 1 }] },
    {
      requests: [
        {
          request_id: 'r',
          am: 'a',
          elapsed_ms: 1,
          agent_id_sha256: hash('agent-one'),
          owner: 'a',
          owner_epoch: 1,
          owner_stable: true,
        },
      ],
    },
    {
      requests: [
        {
          request_id: 'r',
          am: 'b',
          elapsed_ms: 1,
          agent_id_sha256: hash('agent-one'),
          owner: 'a',
          owner_epoch: 0,
          owner_stable: true,
        },
      ],
    },
    { requests: [{ request_id: 'r', am: 'b', elapsed_ms: 1 }] },
  ]) {
    const { e, calls } = fixtureEnvironment({ evidencePatch: patch });
    const r = await executeCase(definition(boundaryCase), row, e);
    assert.equal(r.status, 'FAIL', JSON.stringify(patch));
    assert.deepEqual(calls, ['browser', 'fixture']);
  }
});
test('retained browser observation uses an allowlist, dropping secret-bearing adapter extensions', async () => {
  const { e } = fixtureEnvironment({
    evidencePatch: { token: 'never-retain', headers: { authorization: 'never-retain' } },
  });
  const events = [];
  e.record = async (event) => events.push(event);
  const r = await executeCase(definition(boundaryCase), row, e);
  assert.equal(r.status, 'PASS');
  assert.equal(JSON.stringify(events).includes('never-retain'), false);
});
test('response capture matches exact method/path across proxy alias and dispatches mutation once', async () => {
  const page = new EventEmitter();
  let calls = 0;
  const response = (path, method, status = 200) => ({
    url: () => `http://local.invalid${path}`,
    status: () => status,
    request: () => ({
      method: () => method,
      postData: () => (method === 'POST' ? '{}' : null),
      postDataJSON: () => ({ intent: 'one' }),
    }),
    headers: () => ({ 'x-request-id': 'joined' }),
    json: async () => ({ id: 'original' }),
  });
  const r = await capture(page, 'POST', '/api/v1/agents/a/share-links', async () => {
    calls++;
    page.emit('response', response('/api/agent-manager/v1/agents/other/share-links', 'POST'));
    page.emit('response', response('/api/agent-manager/v1/agents/a/share-links', 'GET'));
    page.emit('response', response('/api/agent-manager/v1/agents/a/share-links', 'POST'));
  });
  assert.equal(calls, 1);
  assert.equal(r.body.id, 'original');
  assert.equal(r.request_id, 'joined');
  assert.equal(page.listenerCount('response'), 0);
  assert.equal(canonicalPath('/auth/connections/check'), '/auth/connections/check');
});
test('uncertain UI action is never replayed and response listener is released on failure', async () => {
  const page = new EventEmitter();
  let calls = 0;
  await assert.rejects(
    capture(page, 'POST', '/api/v1/test', async () => {
      calls++;
      throw Error('uncertain_action');
    }),
    /uncertain_action/,
  );
  assert.equal(calls, 1);
  assert.equal(page.listenerCount('response'), 0);
});
test('empty DELETE response is accepted without trying to parse JSON', async () => {
  const page = new EventEmitter();
  const r = await capture(page, 'DELETE', '/api/v1/test', async () =>
    page.emit('response', {
      url: () => 'http://local.invalid/api/v1/test',
      status: () => 204,
      request: () => ({ method: () => 'DELETE', postData: () => null }),
      headers: () => ({ 'x-request-id': 'delete' }),
      json: async () => {
        throw Error('should_not_parse');
      },
    }),
  );
  assert.equal(r.body, null);
});
test('download hashing hashes exact bytes rather than the serialized Buffer wrapper', () => {
  assert.equal(hash(Buffer.from('abc')), 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad');
});
