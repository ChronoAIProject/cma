import {EventEmitter} from 'node:events';
import test from 'node:test';
import assert from 'node:assert/strict';
import {settingsRead} from './settings-browser.mjs';

// Contract check for final evidence custody, not a substitute for the real UI run.
for (const lateMutation of [false, true]) {
  test('read-only settings finality checks drained browser receipts: late mutation=' + lateMutation, async () => {
    const cleanups = [], errors = [], observations = [];
    let closed = false;
    const current = {revision: 1, settings: {retention: {agent_log_days: 90}}};
    const provider = {take: async () => ({scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true,
      exclusiveSettings: true, faults: {failNextRead() {}}, cleanup: async () => {}})};
    const adapter = {open: async () => ({page: new EventEmitter(), origin: 'http://127.0.0.1:19905',
      switchAM: async () => {}, close: async () => {closed = true;}}),
    evidence: async () => {
      observations.push(closed);
      return {requests: [{request_id: 'fixture-read', serving_am: 'a', method: 'GET'},
        {request_id: 'fixture-preflight', serving_am: 'a', method: 'OPTIONS'},
        ...(closed && lateMutation ? [{request_id: 'fixture-late-write', serving_am: 'b', method: 'PUT'}] : [])]};
    }};
    const ctx = {am: 'a', fixture: name => name === 'browser' ? adapter : provider,
      defer: (_label, callback) => cleanups.push(callback),
      observe: async (_label, callback) => callback(), request: async () => ({body: structuredClone(current)}),
      step: async () => {}, assert: (value, code) => assert.ok(value, code)};
    await settingsRead.run(ctx);
    for (const cleanup of cleanups.toReversed()) {
      try {await cleanup();} catch (error) {errors.push(error);}
    }
    assert.equal(closed, true); assert.ok(observations.length >= 2 && observations.every(Boolean));
    if (lateMutation) assert.ok(errors.some(error => /admin_read_browser_mutated/.test(error.message)));
    else assert.deepEqual(errors, []);
  });
}

for (const reportState of ['pending', 'failed']) {
  test('read-only settings separates drained report admission: ' + reportState, async () => {
    const page = new EventEmitter(), cleanups = [], receipts = [], observations = [];
    let closed = false, active = false;
    const request = {method: () => 'POST', url: () => 'http://127.0.0.1:19905/api/v1/admin/usage/metrics/reports',
      postDataJSON: () => ({from: 1, to: 2})};
    const provider = {take: async () => ({scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true,
      exclusiveSettings: true, faults: {failNextRead() {}}, cleanup: async () => {}})};
    const current = {revision: 1, settings: {retention: {agent_log_days: 90}}};
    const adapter = {open: async () => ({page, origin: 'http://127.0.0.1:19905', switchAM: async () => {},
      close: async () => {
        if (closed) return; closed = true;
        page.emit('request', request);
        page.emit('response', {request: () => request, status: () => 202, headerValue: async () => 'report-prefetch',
          json: async () => ({report_id: 'report-id', status: reportState})});
        receipts.push({method: 'POST', status: 202, request_id: 'report-prefetch', am: 'a', serving_am: 'a'});
      }}), evidence: async () => ({requests: structuredClone(receipts)})};
    // Isolate the registered cleanup contract: no fake execution of UI steps.
    const ctx = {am: 'a', fixture: name => name === 'browser' ? adapter : provider,
      defer: (_label, run) => cleanups.push(run), step: async () => {},
      assert: () => assert.ok(active, 'assertion_outside_step'),
      observe: async (label, run) => {const value = await run(); observations.push({label, value}); return value;},
      request: async () => ({body: structuredClone(current)})};
    await settingsRead.run(ctx);
    const errors = [];
    for (const cleanup of cleanups.toReversed()) try {await cleanup();} catch (error) {errors.push(error);}
    if (reportState === 'pending') assert.deepEqual(errors, []);
    else assert.ok(errors.some(e => e.errors?.some(cause => cause.message === 'admin_operations_report_admission')));
    assert.ok(observations.some(o => o.value?.requests?.some(r => r.request_id === 'report-prefetch')));
    assert.equal(page.listenerCount('request'), 0); assert.equal(page.listenerCount('response'), 0);
  });
}
