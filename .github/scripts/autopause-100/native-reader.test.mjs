import test from 'node:test';
import assert from 'node:assert/strict';
import {createNativeReader} from './native-reader.mjs';
import {fixture, server} from './fixtures/native.mjs';
import {isNativeSuspended} from './lifecycle.mjs';
import {sameHome, campaign} from './engine.mjs';

test('real HTTP reads return bounded suspension evidence without private fields', async t => {
  const live = await server(t);
  const read = createNativeReader({origin: live.origin, binding: live.data.binding});
  const result = await read();
  assert(isNativeSuspended(result));
  sameHome({...live.data.binding, native_uid: live.data.binding.sandbox.uid}, result);
  assert.equal(result.evidence.scope, 'READ_ONLY_OBSERVATION');
  assert.equal(result.evidence.qualification, 'NOT_ASSESSED');
  assert.equal(result.evidence.reads.length, 7);
  assert(result.evidence.reads.every(row => /^[a-f0-9]{64}$/.test(row.sha256)));
  assert(!JSON.stringify(result).includes('omit-this-field'));
  assert(live.requests.every(req => req.method === 'GET'));
  assert(live.requests.every(req => Object.hasOwn(live.data.routes, req.url)));
  assert.equal(live.requests.length, 7);
});

test('a typed PodList without item type fields still reports its live Pod', async t => {
  const data = fixture('Running');
  // The Kubernetes codec puts the type metadata on the list envelope.
  delete data.pod.kind;
  delete data.pod.apiVersion;
  const live = await server(t, data);
  const result = await createNativeReader({origin: live.origin, binding: live.data.binding})();
  assert.equal(result.operating_mode, 'Running');
  assert.deepEqual(result.pods, [{name: 'worker', uid: 'pod-uid', phase: 'Running', deleting: false}]);
  assert.equal(isNativeSuspended(result), false);
  sameHome({...live.data.binding, native_uid: live.data.binding.sandbox.uid}, result);
});

test('configuration and caller mutations cannot redirect the pinned reader', async t => {
  const live = await server(t);
  const binding = structuredClone(live.data.binding);
  const read = createNativeReader({origin: live.origin, binding});
  binding.namespace.name = 'foreign'; binding.pvcs[0].name = 'foreign';
  await read();
  assert(live.requests.every(req => !req.url.includes('foreign')));
  for (const origin of ['https://example.invalid', 'http://localhost:42',
    'http://127.0.0.1:42/path', 'http://user@127.0.0.1:42',
    'http://127.0.0.1:42/?key=value', 'http://127.0.0.1:42/#fragment', 'ws://127.0.0.1:42']) {
    assert.throws(() => createNativeReader({origin, binding: live.data.binding}));
  }
  for (const timeoutMs of [0, -1, NaN, 0.5, 30001, Infinity]) {
    assert.throws(() => createNativeReader({origin: live.origin, binding: live.data.binding, timeoutMs}));
  }
  assert.throws(() => createNativeReader());
});

test('HTTP errors, redirects and invalid bodies produce sanitized failures', async t => {
  for (const status of [301, 302, 307, 308, 401, 403, 404, 429, 500]) {
    await t.test(`HTTP ${status}`, async t => {
      const live = await server(t, fixture(), (_req, res) => {
        res.writeHead(status, {Location: 'http://example.invalid/private'});
        res.end('omit-this-field'); return true;
      });
      await assert.rejects(createNativeReader({origin: live.origin, binding: live.data.binding}),
        error => error.message === 'native_observation_namespace_failed' && !JSON.stringify(error).includes('private'));
      assert.equal(live.requests.length, 1);
    });
  }
  const malformed = await server(t, fixture(), (_req, res) => { res.end('not json'); return true; });
  await assert.rejects(createNativeReader({origin: malformed.origin, binding: malformed.data.binding}),
    /native_observation_namespace_failed/);
});

test('oversized response is refused and a hung body respects the common deadline', async t => {
  const oversized = await server(t, fixture(), (_req, res) => {
    res.end(' '.repeat(4 * 1024 * 1024 + 1)); return true;
  });
  await assert.rejects(createNativeReader({origin: oversized.origin, binding: oversized.data.binding}),
    /native_observation_namespace_failed/);
  const hung = await server(t, fixture(), (_req, res) => {
    res.writeHead(200, {'Content-Type': 'application/json'}); res.write('{'); return true;
  });
  const started = performance.now();
  await assert.rejects(createNativeReader({origin: hung.origin, binding: hung.data.binding, timeoutMs: 100}),
    /native_observation_namespace_failed/);
  assert(performance.now() - started < 3000);
});

test('the total deadline is shared by reads rather than reset at each stage', async t => {
  const timers = [];
  t.after(() => timers.forEach(clearTimeout));
  const live = await server(t, fixture(), (req, res, requests, data) => {
    if (requests.length === 1) {
      timers.push(setTimeout(() => res.end(JSON.stringify(data.routes[req.url])), 120));
    } else {
      // A second independently budgeted request would receive this after 250 ms.
      timers.push(setTimeout(() => res.end(JSON.stringify(data.routes[req.url])), 130));
    }
    return true;
  });
  await assert.rejects(createNativeReader({origin: live.origin, binding: live.data.binding, timeoutMs: 200}),
    /native_observation_(namespace|sandbox)_failed/);
  assert(live.requests.length <= 2);
});

test('a pre-aborted caller sends no request and in-flight cancellation closes observation', async t => {
  const live = await server(t);
  const read = createNativeReader({origin: live.origin, binding: live.data.binding});
  await assert.rejects(read({signal: AbortSignal.abort()}), /native_observation_namespace_failed/);
  assert.equal(live.requests.length, 0);
  const controller = new AbortController();
  const hung = await server(t, fixture(), () => { controller.abort(); return true; });
  await assert.rejects(createNativeReader({origin: hung.origin, binding: hung.data.binding})({signal: controller.signal}),
    /native_observation_namespace_failed/);
  assert.equal(hung.requests.length, 1);
});

test('namespace, Sandbox and PVC drift during recheck refuse the snapshot', async t => {
  for (const target of ['namespace', 'sandbox', 'pvc']) {
    await t.test(target, async t => {
      const live = await server(t, fixture(), (_req, _res, requests, data) => {
        if (requests.length === 5) data[target].metadata.resourceVersion = 'changed';
        return false;
      });
      await assert.rejects(createNativeReader({origin: live.origin, binding: live.data.binding}),
        /native_observation_recheck_failed/);
    });
  }
});

test('foreign resources, truncated lists and remaining Pods never report suspension', async t => {
  const cases = [
    ['namespace', d => { d.namespace.metadata.uid = 'replacement'; }],
    ['sandbox', d => { d.sandbox.metadata.annotations['cma/home-identity'] = 'foreign'; }],
    ['pvcs', d => { d.pvc.spec.volumeName = 'replacement'; }],
    ['pods', d => { d.pods.metadata.continue = 'more'; }],
    ['pods', d => { d.pods.metadata.remainingItemCount = 1; }],
    ['pods', d => { d.pods.items = [d.pod]; }],
    ['pods', d => { d.pod.metadata.deletionTimestamp = '2026-01-01T00:00:00Z'; d.pods.items = [d.pod]; }],
  ];
  for (const [stage, mutate] of cases) {
    const data = fixture(); mutate(data); const live = await server(t, data);
    await assert.rejects(createNativeReader({origin: live.origin, binding: data.binding}),
      new RegExp(`native_observation_${stage}_failed`));
  }
});

test('existing campaign consumes real HTTP snapshots without claiming native qualification', async t => {
  const live = await server(t);
  const read = createNativeReader({origin: live.origin, binding: live.data.binding});
  const baseline = {home: await read()};
  const packets = [];
  const adapter = {origins: {ams: {a: live.origin}, control_origin: live.origin},
    validate: async () => {}, prepare: async () => baseline, alwaysOnException: async () => {},
    awaitAutomaticPause: read,
    wakeWithMessage: async () => {
      const running = fixture('Running');
      live.data.routes = running.routes;
      return {synthetic: true};
    }, home: read,
    holdActiveTurn: async () => ({interrupted: false, samples: 1, hold_ms: 1}),
    completeTurn: async () => ({synthetic: true}), verifyRetention: async () => {},
    verifyAllReceipts: async () => {},
  };
  const result = await campaign(adapter, {count: 1, record: async packet => packets.push(packet)});
  assert.equal(result.verdict, 'PASS');
  assert.equal(result.summary.qualification, 'NOT_ASSESSED');
  assert(isNativeSuspended(result.cycles[0].pause));
  assert.equal(packets.at(-1).final_verification, 'PASS');
  assert(live.requests.every(req => req.method === 'GET'));
});
