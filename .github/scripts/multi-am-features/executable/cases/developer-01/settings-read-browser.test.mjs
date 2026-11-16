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
    const adapter = {open: async () => ({page: {}, origin: 'http://127.0.0.1:19905',
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
