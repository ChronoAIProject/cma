import test from 'node:test';
import assert from 'node:assert/strict';
import {EventEmitter} from 'node:events';
import {failSettingsRead} from './settings-read-fault.mjs';

const path = '/api/v1/admin/settings', origin = 'http://127.0.0.1:19903';
function fixture({abortError, unrouteError, offError, gate, status = 200} = {}) {
  const page = new EventEmitter(), actions = [], cleanups = [];
  let match, handler;
  page.route = async (predicate, callback) => {match = predicate; handler = callback;};
  page.unroute = async (predicate, callback) => {
    assert.equal(predicate, match); assert.equal(callback, handler);
    actions.push('unroute'); if (unrouteError) throw unrouteError;
    handler = null;
  };
  const off = page.off.bind(page);
  page.off = (event, listener) => {off(event, listener); if (offError) throw offError;};
  return {page, actions, cleanups, session: {page, origin},
    ctx: {defer: (_label, callback) => cleanups.push(callback)},
    async dispatch(method = 'GET', url = origin + path) {
      const request = {method: () => method, url: () => url};
      page.emit('request', request);
      if (!handler || !match(new URL(url))) return;
      await handler({request: () => request,
        abort: async reason => {
          actions.push(['abort', reason]); if (gate) await gate; if (abortError) throw abortError;
        },
        fallback: async () => {
          actions.push(['fallback', method]);
          page.emit('response', {request: () => request, status: () => status});
        },
      });
    },
  };
}
const install = f => failSettingsRead(f.ctx, {session: f.session, path});

test('one transport failure is followed by a genuine read response with separate observations', async () => {
  const f = fixture(), fault = await install(f);
  await f.dispatch('GET', origin + '/api/agent-manager/v1/admin/settings');
  await f.dispatch(); await fault.cleanup(); await f.cleanups[0]();
  const receipt = await fault.receipt();
  assert.equal(receipt.failed_reads, 1); assert.equal(receipt.failed_request_index, 0);
  assert.equal(receipt.error_count, 0); assert.equal(receipt.requests.length, 2);
  assert.equal(receipt.requests[0].failure, 'injected_browser_transport_failure');
  assert.equal(receipt.requests[0].status, null); assert.equal(receipt.requests[0].serving_am, null);
  assert.equal(receipt.requests[1].status, 200); assert.equal(receipt.requests[1].failure, undefined);
  for (const request of receipt.requests) {
    assert.equal(request.method, 'GET'); assert.equal(request.path, path);
    assert.ok(Number.isFinite(Date.parse(request.started_at)));
    assert.ok(Number.isFinite(Date.parse(request.ended_at)));
    assert.ok(Number.isFinite(request.duration_ms) && request.duration_ms >= 0);
  }
  assert.deepEqual(f.actions, [['abort', 'failed'], ['fallback', 'GET'], 'unroute']);
  assert.equal(f.page.listenerCount('request') + f.page.listenerCount('response'), 0);
  receipt.requests[0].status = 200;
  assert.equal((await fault.receipt()).requests[0].status, null);
});

test('actual recovery errors are retained without a fabricated successful response', async () => {
  const f = fixture({status: 503}), fault = await install(f);
  await f.dispatch(); await f.dispatch(); await fault.cleanup();
  assert.equal((await fault.receipt()).requests[1].status, 503);
});

test('a mutation is never selected as the failed read and remains visible to the case', async () => {
  const f = fixture(), fault = await install(f);
  await f.dispatch('PUT'); await f.dispatch(); await f.dispatch('PATCH');
  await fault.cleanup();
  const receipt = await fault.receipt();
  assert.equal(receipt.failed_request_index, 1);
  assert.deepEqual(receipt.requests.map(r => r.method), ['PUT', 'GET', 'PATCH']);
  assert.equal(f.actions.filter(a => a[0] === 'abort').length, 1);
});

test('other origins, paths and query strings are neither faulted nor reported', async () => {
  const f = fixture(), fault = await install(f);
  for (const url of ['https://example.invalid' + path, 'http://127.0.0.1:19904' + path,
    origin + path + '/other', origin + path + '?other=true']) await f.dispatch('GET', url);
  await fault.cleanup();
  assert.deepEqual(await fault.receipt(), {
    failed_reads: 0, failed_request_index: null, requests: [], error_count: 0,
  });
  assert.deepEqual(f.actions, ['unroute']);
});

test('nonlocal or incomplete browser bindings fail before installing any listener', async () => {
  for (const badOrigin of ['https://example.invalid', origin + '/other', 'http://user@localhost', 'invalid']) {
    const f = fixture(); f.session.origin = badOrigin;
    await assert.rejects(install(f), /admin_read_fault_local_origin_required/);
    assert.equal(f.cleanups.length, 0); assert.equal(f.page.listenerCount('request'), 0);
  }
  const f = fixture();
  await assert.rejects(failSettingsRead(f.ctx, {session: f.session, path: '/other'}),
    /admin_read_fault_settings_path_required/);
  await assert.rejects(failSettingsRead(f.ctx, {session: {page: {}}, path}),
    /admin_read_fault_browser_binding_missing/);
});

test('cleanup drains an in-flight abort before retaining its final observation', async () => {
  let release;
  const f = fixture({gate: new Promise(resolve => {release = resolve;})}), fault = await install(f);
  const dispatch = f.dispatch(); let done = false;
  const cleanup = fault.cleanup().then(() => {done = true;});
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(done, false); assert.equal(f.page.listenerCount('request'), 1);
  release(); await dispatch; await cleanup;
  assert.equal((await fault.receipt()).failed_reads, 1);
  assert.equal(f.page.listenerCount('request') + f.page.listenerCount('response'), 0);
});

test('abort and removal failures survive every cleanup call without another abort', async () => {
  const abortError = Error('abort failed'), unrouteError = Error('unroute failed'), offError = Error('off failed');
  const f = fixture({abortError, unrouteError, offError}), fault = await install(f);
  await f.dispatch();
  for (let i = 0; i < 2; i++) await assert.rejects(fault.cleanup(), error =>
    [abortError, unrouteError, offError].every(value => error.errors.includes(value)));
  assert.equal((await fault.receipt()).failed_reads, 0);
  assert.equal(f.actions.filter(a => a[0] === 'abort').length, 1);
  assert.equal(f.actions.filter(a => a === 'unroute').length, 1);
  assert.equal(f.page.listenerCount('request') + f.page.listenerCount('response'), 0);
});

test('partial route registration failure retains cleanup of both listeners', async () => {
  const f = fixture();
  f.page.route = async () => {throw Error('route unavailable');};
  f.page.unroute = async () => {};
  await assert.rejects(install(f), /route unavailable/);
  assert.equal(f.cleanups.length, 1);
  await f.cleanups[0]();
  assert.equal(f.page.listenerCount('request') + f.page.listenerCount('response'), 0);
});
