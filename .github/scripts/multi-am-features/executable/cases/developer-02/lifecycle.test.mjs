import test from 'node:test';
import assert from 'node:assert/strict';
import {stopEligible} from './lifecycle.mjs';

test('Stop uses the serialized working status for an active turn', () => {
  assert.equal(stopEligible('working'), true);
  for (const status of ['starting', 'ready', 'waiting_approval', 'paused', 'stopped', 'deleted', 'failed', 'running'])
    assert.equal(stopEligible(status), false, status);
});

import {cases} from './lifecycle.mjs';

test('actual Stop case reaches public Stop for working and separates admission from terminal confirmation', async () => {
  for (const status of ['working', 'running', 'stopped']) {
    const definition = cases.find(c => c.id === 'C275:cma-user.agent.stop');
    const events = [], deferred = [], fixture = {local: true, run_owned: true, createdByRun: true,
      agentId: 'actual-agent', stopBarrier: {release: async () => events.push('release')},
      cleanup: async () => ({complete: true, owned_only: true})};
    const locator = {waitFor: async () => {}, isVisible: async () => true,
      innerText: async () => events.includes('release') ? 'Confirmed' : 'Accepted Waiting for confirmation',
      click: async () => {}, getByRole: (_role, options) => ({...locator,
        click: async () => {if (options.name === 'Stop') events.push('POST stop');}})};
    const response = {request: () => ({method: () => 'POST', postDataJSON: () => ({})}),
      url: () => 'http://local.invalid/api/v1/agents/actual-agent/stop', status: () => 202,
      text: async () => '{}', headers: () => ({'x-request-id': 'stop-actual'})};
    const page = {goto: async () => {}, getByRole: () => locator,
      locator: () => locator, waitForResponse: async predicate => {assert.equal(predicate(response), true); return response;}};
    // Model the actual menu click, not a fake successful stop fixture.
    page.getByRole = (role, options) => ({...locator, click: async () => {
      if (role === 'menuitem' && options.name === 'Stop') events.push('POST stop');
    }});
    const ctx = {row: {case_id: definition.id}, state: {}, am: 'a',
      startAction: boundary => {assert.equal(boundary, 'stop_to_terminal'); events.push('timer-start'); return () => events.push('timer-end');},
      fixture: name => name === 'browser' ? {open: async () => ({page, origin: 'http://local.invalid', close: async () => {}})}
        : {take: async () => fixture}, defer: (label, fn) => deferred.push({label, fn}),
      assert: (value, code) => assert.ok(value, code),
      step: async (step, fn) => {if (step === 'step_3') throw Error('stop_probe_complete'); await fn();},
      request: async query => ({body: query.path.includes('/transcript')
        ? {agent_id: fixture.agentId, turns: [], through_seq: 0, next: null}
        : {id: fixture.agentId, status: events.includes('release') ? 'stopped' : status}})};
    await assert.rejects(definition.run(ctx), status === 'working' ? /stop_probe_complete/ : /stop_starts_working/);
    if (status === 'working') assert.deepEqual(events, ['timer-start', 'POST stop', 'release', 'timer-end']);
    else assert.deepEqual(events, []);
  }
});
