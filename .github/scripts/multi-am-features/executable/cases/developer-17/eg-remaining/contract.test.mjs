import { validEgReceipt } from './receipts.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { readFile } from 'node:fs/promises';
import { cases, setups } from './index.mjs';
import { prepare, names } from './setup.mjs';
import { createTransport } from './transport.mjs';
import {
  IDS,
  catalog,
  uiMutation,
  closeAndCapture,
  validateBrowserEvidence,
  assertIdentity,
} from './core.mjs';
import { Blocked } from '../../../runner.mjs';
import { servingBinding } from '../../../serving.mjs';
import { AUTHORS, ROOT, validateDefinitions } from '../../../registry.mjs';

const ctx = () => ({
  am: 'a',
  row: { case_id: 'C275:EG-020' },
  phase: 'before_restart',
  iteration: 1,
  assert: (condition, code) => assert.ok(condition, code),
  observations: [],
  defers: [],
  async observe(name, read) {
    const value = await read();
    this.observations.push({ name, value });
    return value;
  },
  defer(name, run) {
    this.defers.push({ name, run });
  },
});
async function server(t, handler) {
  const requests = [];
  const server = createServer((req, res) => {
    requests.push({ method: req.method, url: req.url });
    handler(req, res);
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  t.after(
    () =>
      new Promise((resolve) => {
        server.closeAllConnections();
        server.close(resolve);
      }),
  );
  return { origin: 'http://127.0.0.1:' + server.address().port, requests };
}
function parentFor(a, b, topology) {
  return {
    scope: 'LOCAL_KIND_DISPOSABLE_TWO_AM',
    synthetic: true,
    authorize: async () => ({}),
    topology:
      topology ??
      (async () => ({
        egs: {
          a: {
            origin: a,
            ready: true,
            pod_uid: 'eg-a',
            instance_id: 'eg-process-a',
            container_id: 'container-a',
            source: 'a'.repeat(40),
            image: 'eg@sha256:' + 'a'.repeat(64),
          },
          b: {
            origin: b,
            ready: true,
            pod_uid: 'eg-b',
            instance_id: 'eg-process-b',
            container_id: 'container-b',
            source: 'a'.repeat(40),
            image: 'eg@sha256:' + 'a'.repeat(64),
          },
        },
      })),
  };
}
function jsonResponse(res, body = { record_id: 'accepted', channel_id: 'channel', version: 4 }) {
  res.writeHead(201, { 'content-type': 'application/json', 'x-request-id': 'eg-request' });
  res.end(JSON.stringify(body));
}
function receipt(scope = 'eg', am = 'a') {
  const serving = servingBinding(am, {
    ams: {
      [am]: {
        ready: true,
        pod_uid: 'pod-' + am,
        instance_id: 'instance-' + am,
        container_id: 'container-' + am,
        origin: 'http://127.0.0.1:' + (am === 'a' ? 39801 : 39802),
      },
    },
  });
  return {
    scope,
    am,
    request_id: 'request-' + am,
    started_at: serving.observed_at,
    ended_at: serving.observed_at,
    serving,
    serving_after: serving,
    attribution: { verdict: 'PASS', basis: 'pod_forward_before_after' },
  };
}
function egReceipt(replica = 'a') {
  const amReceipt = receipt('am', replica),
    rest = { ...amReceipt };
  delete rest.am;
  const convert = (value) => {
    const snapshot = { ...value };
    delete snapshot.am;
    delete snapshot.schema;
    return {
      ...snapshot,
      schema: 'c275-eg-serving-v1',
      scope: 'eg',
      replica,
      source: 'a'.repeat(40),
      image: 'eg@sha256:' + 'a'.repeat(64),
    };
  };
  return {
    ...rest,
    scope: 'eg',
    replica,
    serving: convert(amReceipt.serving),
    serving_after: convert(amReceipt.serving_after),
  };
}
function evidence(requests = [receipt('am')], eg_requests = [egReceipt()]) {
  return {
    schema: 'multi-eg-browser-evidence-v1',
    compiled_source: 'a'.repeat(40),
    real_ui: true,
    secrets_redacted: true,
    requests,
    eg_requests,
  };
}

// These tests validate authored wiring, not local-kind feature acceptance.
test('exact twenty canonical IDs preserve all original steps including EG-048 step 3', () => {
  assert.deepEqual(
    cases.map((c) => c.id),
    IDS.map((n) => 'C275:EG-' + String(n).padStart(3, '0')),
  );
  for (const item of cases)
    assert.deepEqual(item.steps, catalog.find((row) => row.case_id === item.id).required_steps);
  assert.equal(cases.find((c) => c.id === 'C275:EG-048').steps.length, 4);
  assert.equal(Object.keys(setups).length, 20);
});
test('additive integration respects prod-verifier-03 ownership and reaches exactly 275 C275 plus 87 API', async () => {
  const cat = JSON.parse(await readFile(ROOT + '/catalog.json', 'utf8'));
  const assignments = [],
    contributions = [];
  for (const owner of AUTHORS) {
    assignments.push(JSON.parse(await readFile(ROOT + '/assignments/' + owner + '.json', 'utf8')));
    const imported = await import(ROOT + '/cases/' + owner + '/index.mjs');
    contributions.push({
      owner,
      definitions: [...imported.cases, ...(owner === 'prod-verifier-03' ? cases : [])],
      setupDefinitions: { ...imported.setups, ...(owner === 'prod-verifier-03' ? setups : {}) },
    });
  }
  const result = validateDefinitions(cat, assignments, contributions);
  assert.equal([...result.cases.keys()].filter((id) => id.startsWith('C275:')).length, 275);
  assert.equal([...result.cases.keys()].filter((id) => id.startsWith('API:')).length, 87);
});
for (const name of names)
  test(name + ' setup blocks before any mutation without real parent primitives', async () => {
    const context = ctx();
    await assert.rejects(
      prepare(context, {}, name),
      (error) => error instanceof Blocked && error.message === 'eg20_parent_primitives_required',
    );
    assert.equal(context.defers.length, 0);
  });
test('EG transport uses distinct actual replica origins and preserves full receipts', async (t) => {
  const a = await server(t, (_req, res) => jsonResponse(res));
  const b = await server(t, (_req, res) => jsonResponse(res));
  const context = ctx(),
    request = await createTransport(context, parentFor(a.origin, b.origin));
  const write = await request({ method: 'POST', path: '/v1/channels', allowed: [201] });
  const read = await request({
    path: '/v1/channel-records/accepted',
    replica: 'b',
    allowed: [201],
  });
  assert.equal(a.requests.length, 1);
  assert.equal(b.requests.length, 1);
  assert.ok(validEgReceipt(write.eg_receipt));
  assert.ok(validEgReceipt(read.eg_receipt));
  assert.equal(write.eg_receipt.serving.pod_uid, 'eg-a');
  assert.equal(read.eg_receipt.serving.pod_uid, 'eg-b');
  assert.equal(
    context.observations.find((row) => row.name === 'eg20_http_serving').value.serving_after
      .container_id,
    'container-a',
  );
  assert.equal(write.scope, 'eg');
  assert.equal(read.scope, 'eg');
});
test('AM paths and normalized traversal cannot use the EG transport', async (t) => {
  const a = await server(t, (_req, res) => jsonResponse(res));
  const request = await createTransport(ctx(), parentFor(a.origin, 'http://127.0.0.1:39802'));
  for (const path of [
    '/api/v1/bots',
    '/api/agent-manager/v1/bots',
    '//127.0.0.1/',
    '/v1/%2e%2e/api/v1/bots',
  ])
    await assert.rejects(request({ path }), /eg20_(eg_path|normalized_path)_required/);
  assert.equal(a.requests.length, 0);
});
test('empty cancellation 204 is retained without attempting JSON decoding', async (t) => {
  const a = await server(t, (_req, res) => {
    res.writeHead(204, { 'x-request-id': 'cancel' });
    res.end();
  });
  const request = await createTransport(ctx(), parentFor(a.origin, 'http://127.0.0.1:39802'));
  const result = await request({
    method: 'DELETE',
    path: '/v1/channel-connections/ref',
    allowed: [204],
  });
  assert.equal(result.status, 204);
  assert.equal(result.body, null);
  assert.ok(validEgReceipt(result.eg_receipt));
});
test('accepted identity survives process drift and the mutation is never replayed', async (t) => {
  const a = await server(t, (_req, res) => jsonResponse(res));
  const parent = parentFor(a.origin, 'http://127.0.0.1:39802');
  const original = parent.topology;
  let calls = 0;
  parent.topology = async () => {
    const topology = await original();
    if (++calls >= 3) topology.egs.a.instance_id = 'replacement';
    return topology;
  };
  const request = await createTransport(ctx(), parent);
  await assert.rejects(
    request({ method: 'POST', path: '/v1/channels', allowed: [201] }),
    (error) => {
      assert.equal(error.response.body.record_id, 'accepted');
      assert.equal(error.response.eg_receipt.attribution.verdict, 'UNPROVED');
      assert.equal(error.outcome, 'unknown');
      return true;
    },
  );
  assert.equal(a.requests.length, 1);
});
test('missing request attribution retains accepted identity without PASS', async (t) => {
  const a = await server(t, (_req, res) => {
    res.writeHead(201, { 'content-type': 'application/json' });
    res.end('{"record_id":"accepted"}');
  });
  const request = await createTransport(ctx(), parentFor(a.origin, 'http://127.0.0.1:39802'));
  await assert.rejects(
    request({ method: 'POST', path: '/v1/channels', allowed: [201] }),
    (error) => {
      assert.equal(error.response.body.record_id, 'accepted');
      assert.equal(error.response.eg_receipt.attribution.verdict, 'UNPROVED');
      return true;
    },
  );
  assert.equal(a.requests.length, 1);
});
test('private observation failure cannot discard an accepted response', async (t) => {
  const a = await server(t, (_req, res) => jsonResponse(res));
  const context = ctx();
  context.observe = async () => {
    throw Error('disk');
  };
  const request = await createTransport(context, parentFor(a.origin, 'http://127.0.0.1:39802'));
  await assert.rejects(
    request({ method: 'POST', path: '/v1/channels', allowed: [201] }),
    (error) => {
      assert.equal(error.message, 'eg20_private_receipt_write_failed');
      assert.equal(error.response.body.record_id, 'accepted');
      return true;
    },
  );
  assert.equal(a.requests.length, 1);
});
test('invalid JSON after admitted mutation remains unknown and is not retried', async (t) => {
  const a = await server(t, (_req, res) => {
    res.writeHead(201, { 'x-request-id': 'accepted' });
    res.end('{');
  });
  const request = await createTransport(ctx(), parentFor(a.origin, 'http://127.0.0.1:39802'));
  await assert.rejects(
    request({ method: 'POST', path: '/v1/channels', allowed: [201] }),
    (error) => error.outcome === 'unknown' && error.response.status === 201,
  );
  assert.equal(a.requests.length, 1);
});
test('setup registers disposal before rejecting invalid authority', async () => {
  const context = ctx(),
    order = [];
  const lease = {
    local_kind: true,
    exclusive: true,
    parent_cleanup_barrier: true,
    retainIntent() {},
    capture() {},
    async dispose() {
      order.push('dispose');
    },
    async finality() {
      order.push('finality');
      return { unresolved: 0, in_flight: 0, resources_remaining: 0, private_custody_retained: 0 };
    },
    async release() {
      order.push('release');
    },
  };
  await assert.rejects(
    prepare(
      context,
      {
        eg: parentFor('http://127.0.0.1:39801', 'http://127.0.0.1:39802'),
        egCoverage: {
          scope: 'LOCAL_KIND_DISPOSABLE_TWO_AM',
          synthetic: true,
          reserve: async () => lease,
          openBrowser() {
            throw Error('must not open');
          },
        },
      },
      'create',
    ),
    /eg20_real_authority_required/,
  );
  assert.equal(context.defers.length, 1);
  await context.defers[0].run();
  assert.deepEqual(order, ['dispose', 'finality', 'release']);
});
test('unresolved provider effects prevent partial setup lease release', async () => {
  const context = ctx();
  let released = false;
  const lease = {
    retainIntent() {},
    capture() {},
    dispose: async () => {},
    finality: async () => ({
      unresolved: 1,
      in_flight: 0,
      resources_remaining: 0,
      private_custody_retained: 1,
    }),
    release: async () => {
      released = true;
    },
  };
  await assert.rejects(
    prepare(
      context,
      {
        eg: parentFor('http://127.0.0.1:39801', 'http://127.0.0.1:39802'),
        egCoverage: {
          scope: 'LOCAL_KIND_DISPOSABLE_TWO_AM',
          synthetic: true,
          reserve: async () => lease,
          openBrowser() {},
        },
      },
      'create',
    ),
    /eg20_exclusive_parent_custody_required/,
  );
  await assert.rejects(context.defers[0].run(), /eg20_partial_lease_unresolved/);
  assert.equal(released, false);
});
test('UI response identity is captured before failed status assertion', async () => {
  const order = [];
  const response = {
    request: () => ({
      method: () => 'POST',
      postDataJSON: () => ({ record_id: 'intent' }),
      headers: () => ({ 'idempotency-key': 'original' }),
    }),
    status: () => 201,
    json: async () => ({ record_id: 'accepted' }),
    headers: () => ({ 'x-request-id': 'accepted' }),
  };
  const fx = {
    session: { page: { waitForResponse: async () => response } },
    capture: async (data) => order.push(data),
  };
  await assert.rejects(
    uiMutation(ctx(), fx, {
      method: 'POST',
      path: '/v1/channels',
      action: async () => {},
      allowed: [200],
    }),
    /eg20_ui_mutation_status/,
  );
  assert.equal(order.length, 1);
  assert.equal(order[0].body.record_id, 'accepted');
  assert.equal(order[0].key, 'original');
});
test('browser final evidence follows close and includes late mutation failure', async () => {
  let closed = false;
  const calls = [];
  const session = {
    async close() {
      calls.push('close');
      closed = true;
    },
    async evidence() {
      calls.push('evidence');
      return { ...evidence(), failures: closed ? ['late_unknown_mutation'] : [] };
    },
  };
  assert.doesNotThrow(() => validateBrowserEvidence(evidence()));
  await assert.rejects(
    closeAndCapture(ctx(), session),
    (error) =>
      error instanceof AggregateError && error.errors[0].message === 'eg20_browser_final_failures',
  );
  assert.deepEqual(calls, ['close', 'evidence']);
});
test('both close failure and final evidence failure survive finalization', async () => {
  const session = {
    async close() {
      throw Error('close_failed');
    },
    async evidence() {
      throw Error('late_mutation_failed');
    },
  };
  await assert.rejects(closeAndCapture(ctx(), session), (error) => {
    assert.deepEqual(
      error.errors.map((error) => error.message),
      ['close_failed', 'late_mutation_failed'],
    );
    return true;
  });
});
test('EG browser evidence cannot use an AM-only proxy or label-only receipt', () => {
  assert.throws(
    () => validateBrowserEvidence(evidence([receipt('am')], [])),
    /eg20_browser_eg_requests_required/,
  );
  assert.throws(
    () =>
      validateBrowserEvidence(evidence([], [{ scope: 'eg', replica: 'a', request_id: 'label' }])),
    /eg20_browser_eg_requests_required/,
  );
  assert.doesNotThrow(() => validateBrowserEvidence(evidence([receipt('am', 'b')], [egReceipt()])));
});
test('custody permits multi-revision advancement but rejects identity or backwards generation changes', () => {
  const basis = { record_id: 'record', channel_id: 'channel', version: 5 };
  assert.doesNotThrow(() => assertIdentity(ctx(), { ...basis, version: 9 }, basis));
  assert.throws(
    () => assertIdentity(ctx(), { ...basis, version: 4 }, basis),
    /eg20_exact_channel_revision/,
  );
  assert.throws(
    () => assertIdentity(ctx(), { ...basis, record_id: 'other' }, basis),
    /eg20_exact_channel_revision/,
  );
});

test('factory retains intent before channel creation and cleans via public API after browser drain', async (t) => {
  const order = [];
  let document;
  const handle = async (req, res) => {
    let encoded = '';
    for await (const part of req) encoded += part;
    if (req.method === 'POST') {
      assert.ok(order.at(-1).startsWith('intent:POST:'), 'intent must be retained before dispatch');
      const body = JSON.parse(encoded);
      assert.equal(Object.hasOwn(body, 'routing'), false);
      document = {
        record_id: body.record_id,
        channel_id: 'channel',
        version: 7,
        definition: { name: body.name, creation: { ready: true } },
      };
      order.push('post');
      jsonResponse(res, document);
    } else if (req.method === 'DELETE') {
      assert.ok(order.includes('closed'), 'browser must finish before resource disposal');
      assert.ok(order.at(-1).startsWith('intent:DELETE:'));
      document = undefined;
      order.push('delete');
      res.writeHead(204, { 'x-request-id': 'delete' });
      res.end();
    } else {
      res.writeHead(document ? 200 : 404, {
        'x-request-id': 'read',
        'content-type': 'application/json',
      });
      res.end(JSON.stringify(document ?? { error: 'not_found' }));
    }
  };
  const a = await server(t, handle),
    b = await server(t, handle),
    context = ctx();
  const page = { route: async () => {}, goto: async () => {}, on() {}, off() {} };
  const session = {
    page,
    origin: a.origin,
    switchAM: async () => {},
    switchEG: async () => {},
    close: async () => {
      order.push('closed');
    },
    evidence: async () => ({ ...evidence(), authentication: 'browser_session' }),
  };
  const lease = {
    local_kind: true,
    exclusive: true,
    parent_cleanup_barrier: true,
    agentRequests: async () => [
      {
        ...receipt('am'),
        case_id: context.row.case_id,
        agent_id_sha256: 'a'.repeat(64),
        owner_proof: {
          verdict: 'PASS',
          kind: 'non_owner',
          owner: 'b',
          ingress: 'a',
          owner_epoch: 1,
          request_id: 'request-a',
          agent_sha256: 'a'.repeat(64),
        },
      },
    ],
    authority: {
      ownerSubject: 'owner',
      agentKey: 'private-fixture-key',
      provider: { slug: 'cma' },
      profile: { metadata: { owner: { subject: 'owner' } } },
    },
    async retainIntent(value) {
      order.push('intent:' + value.method + ':' + value.path);
    },
    capture: async () => {},
    dispose: async () => {
      order.push('disposed');
    },
    finality: async () => ({
      unresolved: 0,
      in_flight: 0,
      resources_remaining: document ? 1 : 0,
      private_custody_retained: 0,
    }),
    release: async () => {
      order.push('released');
    },
  };
  const fx = await prepare(
    context,
    {
      eg: parentFor(a.origin, b.origin),
      egCoverage: {
        scope: 'LOCAL_KIND_DISPOSABLE_TWO_AM',
        synthetic: true,
        reserve: async () => lease,
        openBrowser: async () => session,
      },
    },
    'profile',
  );
  assert.equal(fx.channel.version, 7);
  context.assert = () => {
    throw Error('assertion_outside_step');
  };
  await fx.cleanup();
  await fx.cleanup();
  await context.defers[0].run();
  assert.equal(order.filter((item) => item === 'closed').length, 1);
  assert.equal(order.filter((item) => item === 'delete').length, 1);
  assert.equal(order.filter((item) => item === 'released').length, 1);
  assert.ok(context.observations.some((row) => row.name === 'eg20_browser_final_manifest'));
});

test('EG R8 receipt rejects AM aliases, source/image drift and incomplete snapshots', () => {
  const original = egReceipt();
  assert.equal(validEgReceipt(original), true);
  for (const changed of [
    { ...original, am: 'a' },
    { ...original, serving: { ...original.serving, am: 'a' } },
    { ...original, serving_after: { ...original.serving_after, source: 'b'.repeat(40) } },
    {
      ...original,
      serving_after: { ...original.serving_after, image: 'eg@sha256:' + 'b'.repeat(64) },
    },
    { ...original, serving_after: { ...original.serving_after, draining: true } },
    { ...original, serving_after: { ...original.serving_after, container_id: undefined } },
  ])
    assert.equal(validEgReceipt(changed), false);
});
test('browser records EG scope separately and retains actual AM requests without conversion', async () => {
  const context = ctx(),
    value = evidence([receipt('am', 'b')], [egReceipt('a')]);
  await closeAndCapture(context, { close: async () => {}, evidence: async () => value });
  const eg = context.observations.find((row) => row.value.schema === 'multi-eg-request-v1').value
    .receipt;
  assert.equal(eg.replica, 'a');
  assert.equal(eg.am, undefined);
  const am = context.observations.find((row) => row.value.schema === 'multi-am-browser-evidence-v1')
    .value.requests[0];
  assert.equal(am.am, 'b');
  assert.equal(am.scope, 'am');
  assert.equal(am.serving.schema, 'c275-serving-v1');
});
