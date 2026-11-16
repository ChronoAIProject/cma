import test from 'node:test';
import assert from 'node:assert/strict';
import {EventEmitter} from 'node:events';
import {open, captureUsage} from './helpers.mjs';
import {switchReadAM} from './usage-ui.mjs';

for (const evidenceFails of [false, true]) {
  test('ADMIN browser retains final receipts after failed shutdown: ' + evidenceFails, async () => {
    const order = [], deferred = [], observed = [];
    const closeError = Error('shutdown_failed'), evidenceError = Error('receipt_failed');
    const receipt = {requests: [{request_id: 'request-one', am: 'a'}]};
    const session = {page: {}, close: async () => {
      await Promise.resolve(); order.push('closed'); throw closeError;
    }};
    const browser = {open: async () => session, evidence: async () => {
      assert.deepEqual(order, ['closed']); order.push('evidence');
      if (evidenceFails) throw evidenceError;
      return receipt;
    }};
    const ctx = {am: 'a', fixture: () => browser, defer: (_label, fn) => deferred.push(fn),
      observe: async (_label, fn) => observed.push(await fn())};
    await open(ctx);
    await assert.rejects(deferred[0](), error => {
      assert.ok(error instanceof AggregateError);
      assert.deepEqual(error.errors, evidenceFails ? [closeError, evidenceError] : [closeError]);
      return true;
    });
    assert.deepEqual(order, ['closed', 'evidence']);
    assert.deepEqual(observed, evidenceFails ? [] : [receipt]);
  });
}

test('ADMIN cross-AM cleanup retains attribution failure alongside failed shutdown', async () => {
  const deferred = [], order = [], closeError = Error('shutdown_failed');
  const session = {page: {}, switchAM: async am => assert.equal(am, 'b'),
    close: async () => {order.push('closed'); throw closeError;}};
  const ctx = {am: 'a', defer: (_label, fn) => deferred.push(fn), fixture: () => ({evidence: async () => {
    order.push('evidence'); return {requests: [{method: 'GET', am: 'a', request_id: 'one'}]};
  }})};
  await switchReadAM(ctx, session);
  await assert.rejects(deferred[0](), error => {
    assert.ok(error instanceof AggregateError);
    assert.equal(error.errors[0], closeError);
    assert.equal(error.errors[1].message, 'usage_actual_both_am_reads');
    return true;
  });
  assert.deepEqual(order, ['closed', 'evidence']);
});

for (const closeFails of [false, true]) {
  test('usage response arriving during shutdown is joined: ' + closeFails, async () => {
    const page = new EventEmitter(), closeError = Error('shutdown_failed');
    let resolveBody, finished = false;
    const body = new Promise(resolve => {resolveBody = resolve;});
    const session = {page, close: async () => {
      page.emit('response', {url: () => 'http://local.invalid/api/v1/admin/usage/metrics/reports/view',
        json: () => body, request: () => ({method: () => 'GET', postDataJSON: () => null}),
        status: () => 200, headers: () => ({'x-request-id': 'late-report'})});
      if (closeFails) throw closeError;
    }};
    const capture = captureUsage(session);
    const closing = capture.close().finally(() => {finished = true;});
    const outcome = closeFails ? assert.rejects(closing, error => {
      assert.ok(error instanceof AggregateError); assert.deepEqual(error.errors, [closeError]); return true;
    }) : closing;
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(finished, false);
    resolveBody({report_id: 'report-one'}); await outcome;
    assert.equal(capture.rows.length, 1);
    assert.equal(capture.rows[0].request_id, 'late-report');
    assert.equal(page.listenerCount('response'), 0);
  });
}

test('usage capture and shutdown errors are both retained and pending capture is joined', async () => {
  const page = new EventEmitter(), closeError = Error('shutdown_failed');
  const session = {page, close: async () => {
    page.emit('response', {url: () => 'http://local.invalid/api/v1/admin/usage/metrics/reports/view',
      json: async () => null, request: () => {throw Error('response_read_failed');}});
    throw closeError;
  }};
  const capture = captureUsage(session);
  await assert.rejects(capture.close(), error => {
    assert.ok(error instanceof AggregateError);
    assert.equal(error.errors[0], closeError);
    assert.equal(error.errors[1].message, 'usage_response_capture_failed'); return true;
  });
  assert.equal(page.listenerCount('response'), 0);
});
