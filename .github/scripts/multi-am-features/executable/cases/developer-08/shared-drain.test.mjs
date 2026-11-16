import test from 'node:test';
import assert from 'node:assert/strict';
import {EventEmitter} from 'node:events';
import {setImmediate} from 'node:timers/promises';
import {executeCase} from '../../runner.mjs';
import {setup as setup17, browserEvidence} from '../developer-17/helpers.mjs';
import {open as open16, finish} from '../developer-16/helpers.mjs';
import {session as open14, evidence as evidence14} from '../developer-14/common.mjs';

const source = 'b'.repeat(40), image = 'local/api@sha256:' + 'a'.repeat(64);
const families = [
  {name: 'developer17', close: 'browser_close', retain: 'retain_browser_evidence',
    async open(ctx) {const state = {scope: 'browser'}; await setup17(ctx, state, 'fixture'); return state;},
    observe: browserEvidence},
  {name: 'developer16', close: 'close_browser_session', retain: 'retain_browser_receipt',
    async open(ctx) {ctx.state.f = {webSource: source}; return open16(ctx);},
    observe: ctx => finish(ctx)},
  {name: 'developer14', close: 'configuration_browser_close', retain: 'retain_configuration_browser',
    open: ctx => open14(ctx, {agentId: 'owned'}), observe: evidence14},
];

async function run(family, options = {}) {
  const events = []; let closed = false;
  const page = Object.assign(new EventEmitter(), {
    url: () => 'http://local.test/agents/owned',
    goto: async () => {if (options.openError) throw Error('initial_navigation_failed');},
    getByRole: () => ({waitFor: async () => {}}),
  });
  const response = () => ({url: () => 'http://local.test/api/v1/usage/metrics/view',
    request: () => ({method: () => 'GET', timing: () => ({responseEnd: 5})}),
    status: () => 200, finished: async () => {},
    headers: () => ({'content-type': 'application/json', 'x-request-id': 'late-usage'}),
    json: async () => {await setImmediate(); events.push('trace_body'); return {};}});
  const session = {page, origin: 'http://local.test', switchAM: async () => {},
    async close() {
      events.push('close_start');
      // The browser adapter can only settle a pending mutation at drain.
      await setImmediate(); closed = true;
      if (options.traceResponse) page.emit('response', response());
      events.push('close_join');
      if (options.closeError) throw Error('browser_close_failed');
    }};
  const browser = {open: async () => session,
    async evidence() {
      events.push(closed ? 'final_evidence' : 'early_evidence');
      if (closed && options.evidenceError) throw Error(options.evidenceError);
      const requests = [{method: 'GET', am: 'a', request_id: 'initial', elapsed_ms: 1}];
      if (closed) requests.push({method: 'POST', am: 'b', elapsed_ms: 2,
        request_id: options.attributionError ? undefined : 'late-mutation'});
      return {schema: 'multi-am-browser-evidence-v1', real_ui: true, secrets_redacted: true,
        compiled_source: source, compiled_manifest: 'sha256:' + 'c'.repeat(64), requests,
        ...(closed ? options.finalPatch : {})};
    }};
  const restore = async () => {
    events.push('restore'); assert.equal(closed, false, 'restore must use the live page');
    if (options.restoreError) throw Error('fault_restore_failed');
  };
  const fixtures = {browser, 'user-fixtures': {async take(_name, ctx) {
    if (options.fixtureRestore) ctx.defer('restore_navigation_read_fault', restore);
    if (options.takeError) throw Error('fixture_take_failed');
    return {cleanup: async () => events.push('fixture_cleanup')};
  }}};
  const row = {case_id: 'shared_drain_' + family.name, required_steps: ['action'], family: 'user'};
  const definition = {id: row.case_id, scope: 'browser', steps: row.required_steps,
    fixtures: ['browser', 'user-fixtures'], async run(ctx) {
      await ctx.step('action', async () => {
        const opened = await family.open(ctx);
        if (!options.fixtureRestore) ctx.defer('restore_test_fault', restore);
        if (options.traceError) opened.trace.finish = async () => {
          events.push('trace_error'); throw Error('usage_trace_failed');
        };
        await family.observe(ctx, opened);
        if (options.primaryError) throw Error('primary_action_failed');
        ctx.assert(true, 'action_completed');
      });
    }};
  const result = await executeCase(definition, row, {phase: 'before_restart', iteration: 1,
    am: 'a', image, source, fixtures});
  return {result, events, page};
}

for (const family of families) {
  test(`${family.name}: closes and joins before retaining the late mutation receipt`, async () => {
    const {result, events} = await run(family);
    assert.equal(result.status, 'PASS');
    assert.ok(result.requests.some(row => row.request_id === 'late-mutation'));
    assert.ok(events.indexOf('restore') < events.indexOf('close_start'));
    assert.ok(events.indexOf('close_join') < events.indexOf('final_evidence'));
    assert.equal(result.cleanup.find(row => row.label === family.retain)?.status, 'PASS');
  });
  test(`${family.name}: unknown mutation discovered only at drain cannot pass`, async () => {
    const {result} = await run(family, {evidenceError: 'browser_mutation_outcome_unknown'});
    assert.equal(result.status, 'FAIL');
    assert.equal(result.cleanup.find(row => row.label === family.retain)?.error_code,
      'browser_mutation_outcome_unknown');
  });
  test(`${family.name}: late request attribution is validated`, async () => {
    const {result} = await run(family, {attributionError: true});
    assert.equal(result.status, 'FAIL');
    assert.equal(result.cleanup.find(row => row.label === family.retain)?.error_code,
      'browser_attribution_missing');
  });
  test(`${family.name}: action, close and evidence errors all survive`, async () => {
    const {result, events} = await run(family, {primaryError: true, closeError: true,
      evidenceError: 'browser_mutation_outcome_unknown'});
    assert.equal(result.primary_failure, 'primary_action_failed');
    assert.equal(result.cleanup.find(row => row.label === family.close)?.error_code, 'browser_close_failed');
    assert.equal(result.cleanup.find(row => row.label === family.retain)?.error_code,
      'browser_mutation_outcome_unknown');
    assert.ok(events.indexOf('close_join') < events.indexOf('final_evidence'));
  });
  test(`${family.name}: a failed restoration cannot skip browser close or final evidence`, async () => {
    const {result, events} = await run(family, {restoreError: true});
    assert.equal(result.status, 'FAIL');
    assert.ok(result.cleanup.some(row => row.error_code === 'fault_restore_failed'));
    assert.ok(events.indexOf('restore') < events.indexOf('close_start'));
    assert.ok(events.indexOf('close_join') < events.indexOf('final_evidence'));
  });
  test(`${family.name}: primary failure still retains final successful receipts`, async () => {
    const {result} = await run(family, {primaryError: true});
    assert.equal(result.status, 'FAIL');
    assert.equal(result.primary_failure, 'primary_action_failed');
    assert.ok(result.requests.some(row => row.request_id === 'late-mutation'));
    assert.ok(result.cleanup.every(row => row.status === 'PASS'));
  });
}

test('developer17: fixture-owned fault registered during take is restored before close', async () => {
  const {result, events} = await run(families[0], {fixtureRestore: true});
  assert.equal(result.status, 'PASS');
  assert.equal(events.filter(event => event === 'restore').length, 1);
  assert.ok(events.indexOf('restore') < events.indexOf('close_start'));
  assert.ok(events.indexOf('final_evidence') < events.indexOf('fixture_cleanup'));
});

test('developer17: fixture restoration stays in custody when take fails', async () => {
  const {result, events} = await run(families[0], {fixtureRestore: true, takeError: true});
  assert.equal(result.primary_failure, 'fixture_take_failed');
  assert.deepEqual(events, ['restore']);
  assert.ok(result.cleanup.every(row => row.status === 'PASS'));
});

test('developer17: failure restoring a setup fault is retained without skipping close', async () => {
  const {result, events} = await run(families[0], {fixtureRestore: true, restoreError: true});
  assert.equal(result.status, 'FAIL');
  assert.equal(events.filter(event => event === 'restore').length, 1);
  assert.ok(result.cleanup.some(row => row.error_code === 'fault_restore_failed'));
  assert.ok(events.indexOf('restore') < events.indexOf('close_start'));
  assert.ok(events.indexOf('close_join') < events.indexOf('final_evidence'));
});

for (const [family, finalPatch, error] of [
  [families[0], {compiled_manifest: 'unbound'}, 'actual_compiled_browser_evidence'],
  [families[1], {compiled_source: 'd'.repeat(40)}, 'compiled_real_ui_required'],
  [families[2], {real_ui: false}, 'browser_observation_invalid'],
]) test(`${family.name}: final evidence repeats validation after a valid early observation`, async () => {
  const {result} = await run(family, {finalPatch});
  assert.equal(result.status, 'FAIL');
  assert.equal(result.cleanup.find(row => row.label === family.retain)?.error_code, error);
});

test('developer16: a trace failure cannot skip close or evidence', async () => {
  const {result, events} = await run(families[1], {traceError: true});
  assert.equal(result.status, 'FAIL');
  assert.ok(result.cleanup.some(row => row.error_code === 'usage_trace_failed'));
  assert.ok(events.indexOf('close_join') < events.indexOf('trace_error'));
  assert.ok(events.indexOf('trace_error') < events.indexOf('final_evidence'));
});

test('developer16: primary, close, trace and final evidence failures are independent', async () => {
  const {result} = await run(families[1], {primaryError: true, closeError: true,
    traceError: true, evidenceError: 'browser_mutation_outcome_unknown'});
  assert.equal(result.primary_failure, 'primary_action_failed');
  assert.deepEqual(result.cleanup.filter(row => row.status === 'FAIL').map(row => row.error_code),
    ['browser_close_failed', 'usage_trace_failed', 'browser_mutation_outcome_unknown']);
});

test('developer16: usage trace remains attached during close and joins late response bodies', async () => {
  const {result, events, page} = await run(families[1], {traceResponse: true});
  assert.equal(result.status, 'PASS');
  assert.ok(events.indexOf('close_join') < events.indexOf('trace_body'));
  assert.ok(events.indexOf('trace_body') < events.indexOf('final_evidence'));
  assert.equal(page.listenerCount('response'), 0);
});

test('developer14: navigation failure still closes and captures final evidence', async () => {
  const {result, events} = await run(families[2], {openError: true});
  assert.equal(result.primary_failure, 'initial_navigation_failed');
  assert.ok(result.requests.some(row => row.request_id === 'late-mutation'));
  assert.deepEqual(events, ['close_start', 'close_join', 'final_evidence']);
});
