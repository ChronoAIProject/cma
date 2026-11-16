import test from 'node:test';
import assert from 'node:assert/strict';
import {EventEmitter} from 'node:events';
import {captureMeasured} from './measured-capture.mjs';
const messages = e => [e.message, ...(e.errors ?? []).map(messages)].join(' ');

function captureModel(options = {}) {
  const page = new EventEmitter(), cleanups = [], observed = [], receipts = [], actions = [];
  const ctx = {defer: (_label, run) => cleanups.push(run), observe: async (label, run) => {
    const value = await run(); observed.push({label, value}); return value;
  }, fixture: () => ({evidence: async () => {
    actions.push('evidence'); if (options.evidenceFailure) throw Error('evidence_failed');
    return {authentication: 'browser_session', requests: structuredClone(receipts)};
  }})};
  const session = {page, close: async () => {
    actions.push('close'); await options.onClose?.(emit);
    if (options.closeFailure) throw Error('close_failed');
  }};
  function emit(path, body, extras = {}) {
    const requestId = 'request-' + receipts.length;
    receipts.push({method: extras.method ?? 'GET', status: 200, am: 'a', serving_am: 'a', request_id: requestId});
    page.emit('response', {url: () => 'http://127.0.0.1:1' + path, status: () => 200,
      headerValue: async () => requestId,
      request: () => ({method: () => extras.method ?? 'GET', postDataJSON: () => extras.body}),
      json: async () => typeof body === 'function' ? body() : body});
  }
  const capture = captureMeasured(ctx, session, 'user', 'a', true);
  return {capture, emit, page, cleanups, observed, receipts, actions};
}

test('listener stays attached through close and drains a delayed body before final evidence', async () => {
  let resolveBody, done = false;
  const h = captureModel({onClose: async emit => {
    emit('/api/agent-manager/v1/usage/metrics/reports/one', () => new Promise(resolve => {resolveBody = resolve;}));
  }});
  const closing = h.capture.close().then(() => {done = true;});
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(done, false); assert.equal(h.page.listenerCount('response'), 0);
  resolveBody({report_id: 'one'}); await closing;
  assert.deepEqual(h.actions, ['close', 'evidence']);
  assert.equal(h.capture.rows[0].body.report_id, 'one');
  await h.capture.close(); assert.equal(h.actions.filter(x => x === 'close').length, 1);
});

test('body failure remains sticky across flush and close without unhandled rejection', async () => {
  const h = captureModel(); h.emit('/api/v1/usage/metrics/reports/one', async () => {throw Error('json_failed');});
  await assert.rejects(h.capture.flush(), e => messages(e).includes('json_failed'));
  await assert.rejects(h.capture.close(), e => messages(e).includes('json_failed'));
  assert.equal(h.page.listenerCount('response'), 0);
});

test('close and evidence failures are both retained', async () => {
  const h = captureModel({closeFailure: true, evidenceFailure: true});
  await assert.rejects(h.capture.close(), e => messages(e).includes('close_failed') && messages(e).includes('evidence_failed'));
  assert.deepEqual(h.actions, ['close', 'evidence']);
});

test('capture is bounded and cannot pass with an incomplete response set', async () => {
  const h = captureModel();
  for (let i = 0; i < 257; i++) h.emit('/api/v1/usage/metrics/reports/one', {report_id: 'one'});
  await assert.rejects(h.capture.close(), e => messages(e).includes('admin_measured_capture_bound'));
  assert.equal(h.capture.rows.length, 256);
});

test('auth response capture retains only subject and measured visibility', async () => {
  const h = captureModel(); h.emit('/auth/session', {authenticated: true, subject: 'local-user',
    usage_reporting: {measured_visible: true}, email: 'discard', avatar_url: 'discard', capabilities: ['discard']});
  await h.capture.close();
  assert.deepEqual(h.capture.rows[0].body, {authenticated: true, subject: 'local-user', usage_reporting: {measured_visible: true}});
});

test('unrelated asset response does not become usage evidence', async () => {
  const h = captureModel(); h.emit('/favicon.ico', {});
  await h.capture.close(); assert.equal(h.capture.rows.length, 0);
});
