import test from 'node:test';
import assert from 'node:assert/strict';
import {executeCase} from './runner.mjs';

const row = {case_id: 'C275:budget', family: 'user', required_steps: ['expected_result']};
const environment = () => ({phase: 'before_restart', iteration: 1, am: 'a', source: '1'.repeat(40),
  image: 'local/api@sha256:' + '2'.repeat(64), campaignId: 'budget', caseBudgetMs: 20, cleanupBudgetMs: 1000});
const definition = run => ({id: row.case_id, scope: 'browser', fixtures: ['browser'], steps: row.required_steps, run});

test('deadline closes blocked browser work, joins it and retains a separate cleanup request budget', async () => {
  let interrupt; const calls = [], events = [];
  const env = {...environment(), fixtures: {browser: {open: async () => ({page: {}, close: async () => {
    calls.push('close'); interrupt?.(Error('page_closed'));
  }})}}, record: async event => events.push(event), transport: {request: async request => {
    assert.equal(request.signal.aborted, false); assert.ok(request.timeout > 0 && request.timeout <= 1000);
    calls.push(request.method); return {status: 200, body: {}, timing: {am: request.am, request_id: 'cleanup'}};
  }}};
  const result = await executeCase(definition(async ctx => {
    await ctx.fixture('browser').open({actor: 'user', am: 'a'});
    ctx.defer('public_cleanup', () => ctx.request({method: 'DELETE', path: '/api/v1/owned'}));
    await ctx.step('expected_result', async () => {
      try {await new Promise((_resolve, reject) => {interrupt = reject;});}
      finally {calls.push('work_joined');}
    });
  }), row, env);
  assert.equal(result.status, 'FAIL'); assert.equal(result.error_code, 'case_budget_exhausted');
  assert.equal(result.budget_exhausted, true);
  assert.deepEqual(calls, ['close', 'work_joined', 'DELETE']);
  assert.equal(result.cleanup[0].status, 'PASS');
  assert.ok(events.some(event => event.kind === 'case_result'));
});

test('late accepted writes remain recorded once and do not become a timeout retry', async () => {
  const events = []; let requests = 0;
  const env = {...environment(), caseBudgetMs: 5, record: async event => events.push(event),
    transport: {request: async request => {
      requests++; await new Promise(resolve => setTimeout(resolve, 20));
      return {status: 202, body: {id: 'accepted-once'}, timing: {am: request.am, request_id: 'late'}};
    }}};
  const result = await executeCase({...definition(async ctx => ctx.step('expected_result', async () => {
    const response = await ctx.request({method: 'POST', path: '/api/v1/agents', allowed: [202]});
    ctx.assert(response.body.id === 'accepted-once', 'accepted_original_receipt');
  })), fixtures: []}, row, env);
  assert.equal(result.status, 'FAIL'); assert.equal(result.error_code, 'case_budget_exhausted');
  assert.equal(requests, 1);
  assert.equal(events.find(event => event.kind === 'response').response.body.id, 'accepted-once');
});
