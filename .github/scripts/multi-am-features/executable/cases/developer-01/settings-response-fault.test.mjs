import test from 'node:test';
import assert from 'node:assert/strict';
import {EventEmitter} from 'node:events';
import {withholdSettingsResponse} from './settings-response-fault.mjs';

const path = '/api/v1/admin/settings', origin = 'http://127.0.0.1:19901';
function fixture({status = 200, fetchError, abortError, disposeError, unrouteError, gate} = {}) {
  const page = new EventEmitter(), cleanups = [], actions = [];
  let predicate, handler;
  page.route = async (match, callback) => {predicate = match; handler = callback;};
  page.unroute = async (match, callback) => {
    assert.equal(match, predicate); assert.equal(callback, handler); actions.push('unroute');
    if (unrouteError) throw unrouteError;
    handler = null;
  };
  const session = {page, origin}, ctx = {defer: (_label, callback) => cleanups.push(callback)};
  return {session, ctx, actions, cleanups,
    async dispatch(method, url = origin + path) {
      const request = {method: () => method, url: () => url};
      page.emit('request', request);
      if (!handler || !predicate(new URL(url))) return;
      await handler({request: () => request,
        fetch: async options => {
          actions.push(['fetch', options]); if (gate) await gate;
          if (fetchError) throw fetchError;
          return {status: () => status, dispose: async () => {
            actions.push('dispose'); if (disposeError) throw disposeError;
          }};
        },
        fallback: async () => {actions.push(['fallback', method]);},
        fulfill: async ({response}) => {actions.push(['fulfill', response.status()]);},
        abort: async reason => {actions.push(['abort', reason]); if (abortError) throw abortError;},
      });
    },
  };
}
const install = f => withholdSettingsResponse(f.ctx, {session: f.session, method: 'PUT', path});

test('withholds one genuine successful response and preserves subsequent read order after cleanup', async () => {
  const f = fixture(), fault = await install(f);
  await f.dispatch('GET');
  await f.dispatch('PUT', origin + '/api/agent-manager/v1/admin/settings');
  await f.dispatch('GET'); await f.dispatch('HEAD');
  await fault.cleanup(); await f.cleanups[0]();
  assert.deepEqual(await fault.receipt(), {forwarded_writes: 1, response_status: 200,
    response_withheld: true, error_count: 0,
    following_requests: [{method: 'GET', path}, {method: 'HEAD', path}]});
  assert.deepEqual(f.actions, [['fallback', 'GET'],
    ['fetch', {maxRetries: 0, maxRedirects: 0, timeout: 10000}], ['abort', 'failed'],
    'dispose', ['fallback', 'GET'], ['fallback', 'HEAD'], 'unroute']);
  assert.equal(f.session.page.listenerCount('request'), 0);
});

test('a rejection or redirect is forwarded unchanged and never counted as a withheld success', async () => {
  for (const status of [302, 409, 503]) {
    const f = fixture({status}), fault = await install(f);
    await f.dispatch('PUT'); await fault.cleanup();
    const result = await fault.receipt();
    assert.equal(result.response_status, status); assert.equal(result.response_withheld, false);
    assert.ok(f.actions.some(action => action[0] === 'fulfill' && action[1] === status));
    assert.ok(!f.actions.some(action => action[0] === 'abort'));
  }
});

test('an uncertain forward is not replayed and fails cleanup persistently', async () => {
  const failure = Error('unknown outcome'), f = fixture({fetchError: failure}), fault = await install(f);
  await f.dispatch('PUT');
  assert.deepEqual(await fault.receipt(), {forwarded_writes: 1, response_status: null,
    response_withheld: false, error_count: 1, following_requests: []});
  for (let i = 0; i < 2; i++) await assert.rejects(fault.cleanup(), error => error.errors.includes(failure));
  assert.equal(f.actions.filter(action => action[0] === 'fetch').length, 1);
  assert.equal(f.actions.filter(action => action === 'unroute').length, 1);
});

test('extra browser PUTs remain visible as writes, including ones after the first read', async () => {
  const f = fixture(), fault = await install(f);
  await f.dispatch('PUT'); await f.dispatch('GET'); await f.dispatch('PUT');
  await fault.cleanup();
  const result = await fault.receipt();
  assert.equal(result.forwarded_writes, 2);
  assert.deepEqual(result.following_requests, [{method: 'GET', path}, {method: 'PUT', path}]);
  assert.equal(f.actions.filter(action => action[0] === 'fetch').length, 1);
  assert.ok(f.actions.some(action => action[0] === 'fallback' && action[1] === 'PUT'));
});

test('foreign origins, paths and query-bearing targets are not intercepted or reported', async () => {
  const f = fixture(), fault = await install(f);
  for (const url of ['https://example.invalid' + path, 'http://127.0.0.1:19902' + path,
    origin + path + '/other', origin + path + '?other=true']) await f.dispatch('PUT', url);
  await fault.cleanup();
  assert.deepEqual(await fault.receipt(), {forwarded_writes: 0, response_status: null,
    response_withheld: false, error_count: 0, following_requests: []});
  assert.deepEqual(f.actions, ['unroute']);
});

test('invalid/nonlocal bindings fail before installing a route or listener', async () => {
  for (const badOrigin of ['https://example.invalid', origin + '/other', 'http://user@localhost', 'invalid']) {
    const f = fixture(); f.session.origin = badOrigin;
    await assert.rejects(install(f), /admin_fault_local_origin_required/);
    assert.equal(f.cleanups.length, 0); assert.equal(f.session.page.listenerCount('request'), 0);
  }
  const f = fixture();
  await assert.rejects(withholdSettingsResponse(f.ctx, {session: f.session, method: 'POST', path}),
    /admin_fault_settings_put_required/);
  await assert.rejects(withholdSettingsResponse(f.ctx, {session: {page: {}}, method: 'PUT', path}),
    /admin_fault_browser_binding_missing/);
});

test('cleanup joins an in-flight forward before retaining evidence and removing its listener', async () => {
  let finish;
  const gate = new Promise(resolve => {finish = resolve;});
  const f = fixture({gate}), fault = await install(f);
  const dispatch = f.dispatch('PUT'); let closed = false;
  const cleanup = fault.cleanup().then(() => {closed = true;});
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(closed, false); assert.equal(f.session.page.listenerCount('request'), 1);
  finish(); await dispatch; await cleanup;
  assert.equal((await fault.receipt()).response_withheld, true);
  assert.equal(f.session.page.listenerCount('request'), 0);
});

test('abort, response disposal and unregistration failures all remain in cleanup custody', async () => {
  const abortError = Error('abort failed'), disposeError = Error('dispose failed'),
    unrouteError = Error('unroute failed');
  const f = fixture({abortError, disposeError, unrouteError}), fault = await install(f);
  await f.dispatch('PUT');
  await assert.rejects(fault.cleanup(), error =>
    [abortError, disposeError, unrouteError].every(value => error.errors.includes(value)));
  assert.equal((await fault.receipt()).response_withheld, false);
  assert.equal(f.session.page.listenerCount('request'), 0);
});

test('partial route registration failure still leaves a registered cleanup hook', async () => {
  const f = fixture();
  f.session.page.route = async () => {throw Error('registration failed');};
  let removed = false; f.session.page.unroute = async () => {removed = true;};
  await assert.rejects(install(f), /registration failed/);
  assert.equal(f.cleanups.length, 1);
  await f.cleanups[0]();
  assert.equal(removed, true); assert.equal(f.session.page.listenerCount('request'), 0);
});
