import test from 'node:test';
import assert from 'node:assert/strict';
import {channelUiCases, guardBrowserMutation} from './channel-ui.mjs';

const byId = id => channelUiCases.find(definition => definition.id === `C275:${id}`);

test('EG-022 and EG-031 retain their frozen UI journeys as EG-only evidence', () => {
  for (const [id, fixtures] of [['EG-022', ['browser', 'user-fixtures', 'eg_browser']],
    ['EG-031', ['browser', 'user-fixtures', 'eg_browser']]]) {
    const definition = byId(id);
    assert.ok(definition, `${id} definition`);
    assert.equal(definition.evidence_scope, 'eg_only');
    assert.deepEqual(definition.fixtures, fixtures);
    assert.deepEqual(definition.steps, ['step_1', 'step_2', 'expected_result']);
  }
});

test('channel UI definitions keep the real operation-specific methods distinct', () => {
  const edit = byId('EG-022'), deletion = byId('EG-031');
  assert.notEqual(edit.run, deletion.run);
  assert.match(edit.id, /^C275:EG-022$/);
  assert.match(deletion.id, /^C275:EG-031$/);
});

function receipt(requestId, method, status) {
  const serving = {schema: 'c275-eg-serving-v1', scope: 'eg', replica: 'b',
    origin: 'http://127.0.0.1:4321', ready: true, pod_uid: 'pod-b', instance_id: 'eg-b',
    container_id: 'container-b', source: 'a'.repeat(40), image: 'eg@sha256:' + 'b'.repeat(64),
    observed_at: '2026-11-16T10:00:00.000Z'};
  return {scope: 'eg', replica: 'b', request_id: requestId, method, status, serving,
    serving_after: {...serving, observed_at: '2026-11-16T10:00:00.003Z'},
    started_at: '2026-11-16T10:00:00.001Z', ended_at: '2026-11-16T10:00:00.002Z',
    attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}};
}

async function fakeMutation(method, options = {}) {
  const calls = [], cleanups = [], receipts = [], origin = 'http://127.0.0.1:4321';
  const lease = {channelId: 'channel'};
  const page = {
    async route(_matcher, handler) {this.handler = handler;},
    async unroute() {calls.push('unroute');},
    async dispatch(input = {}) {
      const body = input.body ?? null, headers = input.headers ?? {};
      const request = {method: () => input.method ?? method,
        url: () => origin + (input.path ?? '/api/event-gateway/v1/channels/channel') + (input.search ?? ''),
        postDataJSON: () => body, allHeaders: async () => headers};
      const response = {status: () => options.status ?? (method === 'DELETE' ? 204 : 200),
        headers: () => ({'x-request-id': options.requestId ?? method.toLowerCase() + '-request'}),
        json: async () => options.responseBody ?? {version: 2},
        dispose: async () => calls.push('dispose')};
      return this.handler({request: () => request,
        fetch: async settings => {calls.push(['fetch', settings.maxRetries, settings.maxRedirects]);
          receipts.push(receipt(options.requestId ?? method.toLowerCase() + '-request', method,
            options.status ?? (method === 'DELETE' ? 204 : 200))); return response;},
        fulfill: async () => calls.push('fulfill'), abort: async () => calls.push('abort'),
        fallback: async () => calls.push('fallback')});
    },
  };
  const controller = new AbortController();
  const ctx = {signal: controller.signal, remainingMs: () => 1000,
    assert: (condition, code) => assert.ok(condition, code),
    observe: async (_label, read) => read(), defer: (_label, cleanup) => cleanups.push(cleanup)};
  const session = {page, origin, networkIdle: async () => {}, egEvidence: async () => receipts};
  const guard = await guardBrowserMutation(ctx, session, lease, method, {
    search: options.search ?? '', status: options.status ?? (method === 'DELETE' ? 204 : 200)},
  options.validate ?? (() => {}));
  return {guard, page, calls, cleanups, controller};
}

test('browser mutation guard forwards one exact PUT and joins its serving receipt', async () => {
  const f = await fakeMutation('PUT', {validate: (body, headers, url) => {
    assert.equal(url.search, ''); assert.equal(body.expected_version, 1);
    assert.match(headers['idempotency-key'], /^[0-9a-f-]{36}$/);
  }});
  const result = await f.guard.run(() => f.page.dispatch({body: {expected_version: 1},
    headers: {'idempotency-key': '123e4567-e89b-42d3-a456-426614174000'}}));
  assert.equal(result.status, 200); assert.equal(result.receipt.method, 'PUT');
  assert.deepEqual(f.calls.map(value => Array.isArray(value) ? value[0] : value), ['fetch', 'fulfill', 'dispose']);
  await f.cleanups[0](); assert.equal(f.calls.at(-1), 'unroute');
});

test('browser mutation guard validates exact DELETE query, forbids credentials and rejects duplicates', async () => {
  const f = await fakeMutation('DELETE', {search: '?expected_version=1', validate: (body, headers, url) => {
    assert.equal(body, null); assert.deepEqual(headers, {}); assert.equal(url.search, '?expected_version=1');
  }});
  const input = {search: '?expected_version=1'};
  const result = await f.guard.run(() => f.page.dispatch(input));
  assert.equal(result.status, 204); assert.equal(result.receipt.method, 'DELETE');
  await assert.rejects(f.guard.run(() => f.page.dispatch(input)), /mutation_failed/);
  assert.equal(f.calls.filter(value => Array.isArray(value) && value[0] === 'fetch').length, 1);
  await assert.rejects(f.cleanups[0](), /mutation_cleanup_failed/);
});

test('browser mutation guard aborts wrong revision before forwarding', async () => {
  const f = await fakeMutation('PUT', {validate: (body, _headers, url) => {
    assert.equal(url.pathname, '/api/event-gateway/v1/channels/channel');
    assert.equal(body.expected_version, 1);
  }});
  await assert.rejects(f.guard.run(() => f.page.dispatch({body: {expected_version: 2}})), /mutation_failed/);
  assert.equal(f.calls.some(value => Array.isArray(value) && value[0] === 'fetch'), false);
  await assert.rejects(f.cleanups[0](), /mutation_cleanup_failed/);
});
