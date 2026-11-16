import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { readFileSync } from 'node:fs';
import { cases } from './index.mjs';
import { executeCase, Blocked } from '../../runner.mjs';
import { loadRegistry } from '../../registry.mjs';
import { setup, capture, hash, canonicalPath, open, finish } from './helpers.mjs';
import {
  recipientEffectCount,
  denyAcrossAMs,
  denyOwnerAndRuntimeMutations,
} from './sharing.mjs';
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
function fixtureEnvironment({
  browserFails = false,
  cleanupComplete = true,
  evidencePatch = {},
  closeError,
  evidenceError,
} = {}) {
  const e = env(),
    calls = [];
  const page = new EventEmitter();
  const evidence = {
    schema: 'multi-am-browser-evidence-v1',
    real_ui: true,
    compiled_source: source,
    compiled_manifest: 'sha256:' + '3'.repeat(64),
    secrets_redacted: true,
    requests: [
      {
        request_id: 'request-one',
        am: 'b',
        serving_am: 'b',
        method: 'POST',
        status: 200,
        owner_proof: {
          agent_sha256: hash('agent-one'),
          owner: 'a',
          ingress: 'b',
          kind: 'non_owner',
          owner_epoch: 1,
          request_id: 'request-one',
          verdict: 'PASS',
        },
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
        return {
          page,
          origin: 'http://local.invalid',
          close: async () => {
            calls.push('browser');
            if (closeError) throw Error(closeError);
          },
        };
      },
      evidence: async (selected) => {
        assert.equal(selected, page);
        if (evidenceError) throw Error(evidenceError);
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
  });
}

test('all 34 retained IDs and exact original steps are registered, with no transferred rows', async () => {
  const registered = await loadRegistry();
  assert.equal(cases.length, 34);
  assert.equal([...registered.cases.values()].filter(value => value.owner === 'developer-06').length, 34);
  assert.equal(cases.filter((c) => registered.cases.has(c.id)).length, 34);
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
test('web account flows reject bearer substitution and still close browser and fixture', async () => {
  const { e, calls } = fixtureEnvironment({ evidencePatch: { authentication: 'real_am_bearer' } });
  const r = await executeCase(
    definition(async (ctx) => {
      await ctx.step('step_1', () => setup(ctx, 'contract', 'user', { requireSession: true }));
    }),
    row,
    e,
  );
  assert.equal(r.status, 'BLOCKED');
  assert.equal(r.error_code, 'browser_session_authentication_required');
  assert.deepEqual(calls, ['browser', 'fixture']);
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
test('browser final evidence is captured after session close and delayed mutation failure drain', async () => {
  const page = new EventEmitter(),
    defers = [],
    order = [],
    errors = [];
  let closed = false;
  const ctx = {
    am: 'a',
    state: {},
    fixture(name) {
      if (name === 'user-fixtures') return { webSource: source };
      if (name === 'browser')
        return {
          open: async () => ({
            page,
            origin: 'http://local.invalid',
            close: async () => {
              order.push('session_close');
              closed = true;
              throw Error('session_close_failed');
            },
          }),
          evidence: async () => {
            order.push('evidence');
            if (!closed) throw Error('evidence_before_close');
            throw Error('browser_mutation_outcome_unknown');
          },
        };
      throw Error('unexpected_fixture_' + name);
    },
    defer(label, operation) {
      defers.push({ label, operation });
    },
    observe: async () => undefined,
  };
  const session = await open(ctx, 'user', 'agent-one');
  session.traffic.close = async () => order.push('capture_drain');
  for (const { label, operation } of defers.reverse()) {
    try {
      await operation();
    } catch (error) {
      errors.push({ label, code: error.message });
    }
  }
  assert.deepEqual(order, ['session_close', 'capture_drain', 'evidence']);
  assert.deepEqual(errors, [
    { label: 'close_browser_session', code: 'session_close_failed' },
    { label: 'retain_browser_receipt', code: 'browser_mutation_outcome_unknown' },
  ]);
});
test('primary failure retains browser close and final-evidence failures independently', async () => {
  const { e, calls } = fixtureEnvironment({
    closeError: 'browser_close_failed',
    evidenceError: 'browser_evidence_failed',
  });
  const r = await executeCase(
    definition(async (ctx) => {
      await ctx.step('step_1', async () => {
        await setup(ctx, 'contract');
        ctx.assert(true, 'setup_done');
      });
      await ctx.step('expected_result', async () => {
        ctx.assert(false, 'primary_step_failed');
      });
    }),
    row,
    e,
  );
  assert.equal(r.status, 'FAIL');
  assert.equal(r.primary_failure, 'primary_step_failed');
  assert.deepEqual(
    r.cleanup.map((item) => [item.label, item.error_code ?? null]),
    [
      ['close_browser_session', 'browser_close_failed'],
      ['close_browser_capture', null],
      ['retain_browser_receipt', 'browser_evidence_failed'],
      ['dispose_owned_fixture', null],
    ],
  );
  assert.deepEqual(calls, ['browser', 'fixture']);
});
test('explicit exported finish drains before evidence and preserves every finalization error', async () => {
  const page = {},
    order = [],
    ctx = {
      state: {
        sessions: [
          {
            agentId: 'agent-one',
            session: {
              page,
              close: async () => {
                order.push('session_close');
                throw Error('session_close_failed');
              },
              traffic: {
                close: async () => {
                  order.push('capture_drain');
                  throw Error('capture_drain_failed');
                },
              },
            },
          },
        ],
      },
      fixture(name) {
        if (name === 'user-fixtures') return { webSource: source };
        if (name === 'browser')
          return {
            evidence: async () => {
              order.push('evidence');
              throw Error('browser_evidence_failed');
            },
          };
        throw Error('unexpected_fixture_' + name);
      },
      observe: async () => undefined,
      assert: (condition, code) => assert.equal(condition, true, code),
    };
  await assert.rejects(finish(ctx), (error) => {
    assert.equal(error.name, 'AggregateError');
    assert.deepEqual(error.errors.map((item) => item.message), [
      'session_close_failed',
      'capture_drain_failed',
      'browser_evidence_failed',
    ]);
    return true;
  });
  assert.deepEqual(order, ['session_close', 'capture_drain', 'evidence']);
});
test('sharing permission probes require an effect counter and both ingress denials', async () => {
  const f = { effectJournal: async () => ({ recipient_sends: 0 }) };
  assert.equal(await recipientEffectCount(f), 0);
  await assert.rejects(
    recipientEffectCount({ effectJournal: async () => ({ post_revocation_sends: 0 }) }),
    /recipient_effect_count_missing/,
  );
  const calls = [];
  const ctx = {
    am: 'a',
    state: { f },
    assert(condition, code) {
      assert.equal(condition, true, code);
    },
    request: async (request) => {
      calls.push(request);
      const read = request.method === 'GET';
      return read
        ? { status: 200, body: { capabilities: ['view_details'] }, timing: { am: request.am, request_id: 'read-' + request.am } }
        : { status: 403, body: {}, timing: { am: request.am, request_id: 'write-' + request.am } };
    },
  };
  await denyAcrossAMs(ctx, 'recipient', 'agent-one', 'view_only');
  assert.deepEqual(calls.map((request) => request.am), ['a', 'b']);
  calls.length = 0;
  await denyOwnerAndRuntimeMutations(ctx, 'recipient', 'agent-one', 'read_only');
  assert.deepEqual(
    calls.map((request) => `${request.am}:${request.method}:${request.path}`),
    [
      'a:GET:/api/v1/resource-access/agent/agent-one',
      'a:PATCH:/api/v1/agents/agent-one/preferences',
      'a:POST:/api/v1/agents/agent-one/browser/control',
      'b:GET:/api/v1/resource-access/agent/agent-one',
      'b:PATCH:/api/v1/agents/agent-one/preferences',
      'b:POST:/api/v1/agents/agent-one/browser/control',
    ],
  );
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
