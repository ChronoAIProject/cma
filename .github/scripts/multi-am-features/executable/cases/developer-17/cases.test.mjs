import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { EventEmitter } from 'node:events';
import { cases } from './index.mjs';
import { loadRegistry } from '../../registry.mjs';
import { Blocked, executeCase } from '../../runner.mjs';
import {
  browserEvidence,
  define,
  digest,
  setup,
  transcript,
  watchWrites,
  peer,
} from './helpers.mjs';
import { sendAction, turn, verifyTurnPeer } from './turn-helpers.mjs';
import { admit, inventory, sameInventory } from './creation-helpers.mjs';

// These are offline harness regressions. They do not count as C275 feature PASS.
const assignment = JSON.parse(
  await readFile(new URL('../../assignments/developer-17.json', import.meta.url)),
);
const registry = await loadRegistry();
const image = 'localhost/test@sha256:' + 'a'.repeat(64),
  source = 'b'.repeat(40);
const environment = (fixtures) => ({
  phase: 'before_restart',
  iteration: 1,
  am: 'a',
  source,
  image,
  fixtures,
  record: async () => {},
  transport: {
    request: async () => {
      throw Error('unexpected_network');
    },
  },
});
function context(overrides = {}) {
  const observations = [],
    cleanups = [];
  return {
    am: 'a',
    observations,
    cleanups,
    assert(value, code) {
      assert.ok(value, code);
    },
    observe: async (label, operation) => {
      const value = await operation();
      observations.push({ label, value });
      return value;
    },
    defer: (label, operation) => cleanups.push({ label, operation }),
    ...overrides,
  };
}

test('exact first35 assignment and every original required step; no transferred IDs', () => {
  assert.equal(cases.length, 35);
  assert.deepEqual(cases.map((item) => item.id).sort(), [...assignment.case_ids].sort());
  for (const item of cases) {
    assert.deepEqual(item.steps, registry.rows.get(item.id).required_steps);
    assert.deepEqual(item.fixtures, ['browser', 'user-fixtures']);
  }
});

test('all35 cases fail closed as BLOCKED without browser/setup, before any HTTP request', async () => {
  for (const item of cases) {
    const result = await executeCase(item, registry.rows.get(item.id), environment({}));
    assert.equal(result.status, 'BLOCKED', item.id);
    assert.equal(result.requests.length, 0);
    assert.equal(result.steps.length, 0);
  }
});

test('named setup absence stays BLOCKED even when browser adapter is installed', async () => {
  let opened = 0;
  for (const item of cases) {
    const result = await executeCase(
      item,
      registry.rows.get(item.id),
      environment({
        browser: {
          open: async () => {
            opened += 1;
          },
        },
        'user-fixtures': {
          take: async () => {
            throw new Blocked('named_setup_unavailable');
          },
        },
      }),
    );
    assert.equal(result.status, 'BLOCKED', item.id);
    assert.equal(result.primary_failure, 'named_setup_unavailable');
  }
  assert.equal(opened, 0);
});

test('setup takes cleanup custody before opening a browser; failure cannot leak the fixture', async () => {
  const cleaned = [];
  const def = define('offline-custody', 'browser', [
    async (ctx, s) => setup(ctx, s, 'fixture'),
    async (ctx) => ctx.assert(true, 'unused'),
  ]);
  const row = { case_id: def.id, family: 'user', required_steps: def.steps };
  const result = await executeCase(
    def,
    row,
    environment({
      'user-fixtures': { take: async () => ({ cleanup: async () => cleaned.push('fixture') }) },
      browser: {
        open: async () => {
          throw Error('browser_open_failed');
        },
      },
    }),
  );
  assert.equal(result.status, 'FAIL');
  assert.equal(result.primary_failure, 'browser_open_failed');
  assert.deepEqual(cleaned, ['fixture']);
  assert.equal(result.cleanup[0].status, 'PASS');
});

test('peer switch preserves the existing session and targets distinct ingress; alias is refused', async () => {
  const calls = [],
    ctx = context();
  const state = {
    browser: { otherAM: () => 'b', evidence: async () => ({ requests: [] }) },
    session: { switchAM: async (am) => calls.push(['switch', am]) },
    page: {
      reload: async () => calls.push(['reload']),
      url: () => 'https://local.test/agents/exact',
    },
  };
  await peer(ctx, state);
  assert.deepEqual(calls, [['switch', 'b'], ['reload']]);
  state.browser.otherAM = () => 'a';
  await assert.rejects(peer(ctx, state), /distinct_peer_ingress/);
  assert.equal(calls.length, 2);
});

test('peer read follows actual forwarded write ingress, not the requested ingress label', async () => {
  const calls = [],
    ctx = context();
  const state = {
    data: { agentId: 'owned' },
    browser: {
      otherAM: (am) => (am === 'a' ? 'b' : 'a'),
      evidence: async () => ({
        requests: [{ method: 'POST', am: 'b', agent_id_sha256: digest('owned') }],
      }),
    },
    session: { switchAM: async (am) => calls.push(am) },
    page: { reload: async () => {}, url: () => 'http://local.test/agents/owned' },
  };
  await peer(ctx, state);
  assert.deepEqual(calls, ['a']);
  assert.equal(state.readAm, 'a');
  assert.equal(
    ctx.observations.find((row) => row.label === 'peer_browser_ingress').value.from,
    'b',
  );
});

function sseState(headers = {}) {
  const request = {
    method: () => 'POST',
    headers: () => ({ 'idempotency-key': 'original-key', authorization: 'never-retain' }),
    postDataJSON: () => ({ input: 'hello', stream: true }),
  };
  const response = {
    request: () => request,
    url: () => 'https://local.test/api/v2/agents/owned/responses',
    headers: () => ({
      'x-cma-response-id': 'resp-one',
      'x-cma-command-id': 'cmd-one',
      'x-request-id': 'request-one',
      'set-cookie': 'never-retain',
      ...headers,
    }),
    status: () => 200,
    json: () => {
      throw Error('sse_must_not_be_decoded_as_json');
    },
  };
  return {
    data: { agentId: 'owned' },
    page: {
      waitForResponse: async (predicate) => {
        assert.ok(predicate(response));
        return response;
      },
    },
  };
}
test('Send uses SSE header identity without reading an aborted stream or retaining auth headers', async () => {
  const ctx = context(),
    state = sseState();
  let clicks = 0;
  const receipt = await sendAction(ctx, state, async () => {
    clicks += 1;
  });
  assert.equal(clicks, 1);
  assert.equal(receipt.commandId, 'cmd-one');
  assert.equal(receipt.responseId, 'resp-one');
  assert.equal(receipt.key, 'original-key');
  assert.ok(!JSON.stringify(ctx.observations).includes('never-retain'));
});
test('an accepted SSE without both receipt identities is a failure, never synthetic completion', async () => {
  await assert.rejects(
    sendAction(context(), sseState({ 'x-cma-command-id': undefined }), async () => {}),
    /sse_admission_receipt/,
  );
});

test('public transcript requires the exact Agent and durable turns, not a loose messages array', async () => {
  const state = { data: { agentId: 'owned' } };
  let last;
  const ctx = context({
    request: async (args) => {
      last = args;
      return { body: { agent_id: 'owned', turns: [] } };
    },
  });
  assert.deepEqual(await transcript(ctx, state), []);
  assert.equal(last.agentId, 'owned');
  ctx.request = async () => ({ body: { agent_id: 'foreign', turns: [] } });
  await assert.rejects(transcript(ctx, state), /transcript_turns_shape/);
  ctx.request = async () => ({ body: { agent_id: 'owned', turns: [], next: { through: 99 } } });
  await assert.rejects(transcript(ctx, state), /fixture_transcript_not_truncated/);
});

test('a selected-ingress switch cannot substitute for actual distinct serving-AM readback', async () => {
  const receipt = {
    method: 'GET',
    am: 'b',
    agent_id_sha256: digest('owned'),
    owner_stable: true,
    request_id: 'r1',
  };
  const evidence = {
    schema: 'multi-am-browser-evidence-v1',
    real_ui: true,
    compiled_source: source,
    compiled_manifest: 'sha256:' + 'c'.repeat(64),
    secrets_redacted: true,
    requests: [receipt, { ...receipt, request_id: 'r2' }],
  };
  const state = {
    scope: 'agent',
    peerRequested: true,
    data: { agentId: 'owned' },
    page: {},
    browser: { evidence: async () => evidence },
  };
  await assert.rejects(browserEvidence(context(), state), /actual_agent_reads_on_both_ams/);
  evidence.requests[1].am = 'a';
  await browserEvidence(context(), state);
  evidence.requests[1].owner_stable = false;
  await assert.rejects(browserEvidence(context(), state), /browser_agent_request_attribution/);
});

test('account authentication refuses injected bearer fixtures and cleans the browser', async () => {
  const closed = [],
    def = define('offline-auth', 'browser', [
      async (ctx, s) => setup(ctx, s, 'account-authentication'),
      async (ctx) => ctx.assert(true, 'unused'),
    ]);
  const row = { case_id: def.id, family: 'user', required_steps: def.steps };
  const result = await executeCase(
    def,
    row,
    environment({
      'user-fixtures': { take: async () => ({ cleanup: async () => closed.push('fixture') }) },
      browser: {
        open: async () => ({
          origin: 'http://127.0.0.1:1',
          page: { url: () => 'http://127.0.0.1:1/' },
          switchAM: async () => {},
          close: async () => closed.push('browser'),
        }),
        evidence: async () => ({ authentication: 'real_am_bearer' }),
      },
    }),
  );
  assert.equal(result.status, 'BLOCKED');
  assert.equal(result.primary_failure, 'browser_session_authentication_required');
  assert.deepEqual(closed, ['browser', 'fixture']);
});
test('a terminal failure or duplicate owner-message cannot satisfy completed turn', async () => {
  const state = { data: { agentId: 'owned' } };
  const row = { owner_message: 'unique', turn_sequence: 3, outcome: { type: 'failed' } };
  const ctx = context({ request: async () => ({ body: { agent_id: 'owned', turns: [row] } }) });
  await assert.rejects(turn(ctx, state, 'unique', 'completed'), /expected_terminal_outcome/);
  ctx.request = async () => ({
    body: {
      agent_id: 'owned',
      turns: [
        { ...row, outcome: { type: 'completed' } },
        { ...row, outcome: { type: 'completed' } },
      ],
    },
  });
  await assert.rejects(turn(ctx, state, 'unique', 'completed'), /turn_exactly_once/);
});

test('writer witness excludes unrelated routes and keeps repeated same-key attempts distinct', async () => {
  const page = new EventEmitter(),
    ctx = context(),
    state = { page };
  const writes = watchWrites(ctx, state, '/api/v1/agents');
  const req = (path) => ({
    method: () => 'POST',
    url: () => 'https://local.test' + path,
    headers: () => ({ 'idempotency-key': 'stable' }),
    postDataJSON: () => ({ name: 'test' }),
  });
  page.emit('request', req('/auth/login'));
  page.emit('request', req('/api/v1/agents'));
  page.emit('request', req('/api/v1/agents'));
  assert.equal(writes.length, 2);
  assert.deepEqual(
    writes.map((row) => row.key),
    ['stable', 'stable'],
  );
  await ctx.cleanups[0].operation();
  assert.equal(page.listenerCount('request'), 0);
});

test('admission receipt enters cleanup custody before any later opening/readback step', async () => {
  const held = [],
    ctx = context();
  const response = {
    request: () => ({ method: () => 'POST', postDataJSON: () => ({ name: 'task' }) }),
    url: () => 'https://local.test/api/agent-manager/v1/agents',
    json: async () => ({ id: 'operation-one' }),
    status: () => 202,
    headers: () => ({ 'x-request-id': 'admission-request' }),
  };
  const state = {
    data: { custody: async (receipt) => held.push(receipt) },
    page: {
      waitForResponse: async (predicate) => {
        assert.ok(predicate(response));
        return response;
      },
      getByRole: () => ({ click: async () => {} }),
    },
  };
  await admit(ctx, state);
  assert.deepEqual(held, [{ id: 'operation-one' }]);
});

test('inventory refusal proof rejects truncated pages or any additional resource', async () => {
  const ctx = context({ request: async () => ({ body: { items: [], next: 'more' } }) });
  await assert.rejects(inventory(ctx), /fixture_inventory_bounded/);
  assert.throws(
    () => sameInventory(ctx, { sandboxIds: ['old'] }, { sandboxIds: ['old', 'leaked'] }),
    /refusal_did_not_admit_resources/,
  );
});

test('cross-AM durable success still fails if recovery substituted an admission key', async () => {
  const value = { owner_message: 'message', turn_sequence: 1, outcome: { type: 'completed' } };
  const state = {
    data: { agentId: 'owned' },
    browser: { otherAM: () => 'b', evidence: async () => ({ requests: [] }) },
    session: { switchAM: async () => {} },
    page: { url: () => 'https://local.test/agents/owned', goto: async () => {} },
    writes: [{ key: 'new-key', body: { input: 'message' } }],
  };
  const ctx = context({
    request: async (args) => ({
      body: args.path.includes('/transcript')
        ? { agent_id: 'owned', turns: [value] }
        : args.path.includes('/commands/')
          ? {
              receipt: { agent_id: 'owned', command_id: 'cmd', response_id: 'resp' },
              turn_sequence: 1,
            }
          : { id: 'owned' },
    }),
  });
  await assert.rejects(
    verifyTurnPeer(
      ctx,
      state,
      { commandId: 'cmd', responseId: 'resp', key: 'original' },
      'message',
      'completed',
    ),
    /turn_keeps_admission_identity/,
  );
});
