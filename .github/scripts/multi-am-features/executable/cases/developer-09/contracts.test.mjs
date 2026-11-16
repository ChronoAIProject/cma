import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {executeCase} from '../../runner.mjs';
import {loadRegistry} from '../../registry.mjs';
import {cases} from './index.mjs';
import {completeReport, readyFrom, pages, take} from './helpers.mjs';
const expected = {sandbox_count: 3, allocated_period: {cpu_core_hours: 2, memory_gib_hours: 4, disk_gib_hours: 6}};
const report = () => ({status: 'ready', report_complete: true, report_id: 'report-1',
  scope: {principal: 'operator', admin: true}, summary: structuredClone(expected), query: {from: 0, to: 86400}});

test('report acceptance rejects pending, partial, wrong principal and invalid quantities', () => {
  assert.equal(completeReport(report(), expected, 'operator'), true);
  for (const mutate of [r => r.status = 'pending', r => r.report_complete = false,
    r => r.scope.admin = false, r => r.scope.principal = 'other', r => r.report_id = '',
    r => r.summary.sandbox_count = 4, r => r.summary.allocated_period.cpu_core_hours = null,
    r => r.summary.allocated_period.disk_gib_hours = 'Infinity', r => r.summary.allocated_period.memory_gib_hours = -1]) {
    const r = report(); mutate(r); assert.equal(completeReport(r, expected, 'operator'), false);
  }
  assert.equal(completeReport(report(), {sandbox_count: 3, allocated_period: {}}, 'operator'), false);
});
test('background warm response cannot replace selected range descriptor', () => {
  const wanted = report(), background = report(); background.report_id = 'other'; background.query.to = 30 * 86400;
  assert.equal(readyFrom([{body: wanted}, {body: {report: background}}], '24h').report_id, wanted.report_id);
});
test('every implemented row has original steps and unavailable fixtures block before writes', async () => {
  const registry = await loadRegistry();
  for (const definition of cases) {
    const row = registry.rows.get(definition.id);
    assert.deepEqual(definition.steps, row.required_steps);
    assert.equal(registry.owners.get(definition.id), 'developer-09');
    const result = await executeCase(definition, row, {phase: 'before_restart', iteration: 1,
      am: 'a', image: 'local/api@sha256:' + 'a'.repeat(64), source: 'b'.repeat(40), fixtures: {}});
    assert.equal(result.status, 'BLOCKED'); assert.equal(result.requests.length, 0);
  }
});
test('cross-AM cursor traversal rejects cycles, missing and duplicate records', async () => {
  const seen = [], ctx = {am: 'a', assert: (ok, code) => assert.ok(ok, code),
    request: async arg => {seen.push(arg); return {timing: {am: arg.am, request_id: 'test-request'}, body: seen.length === 1
      ? {items: [{id: 'one'}], next: 'cursor'} : {items: [{id: 'two'}], next: null}};}};
  assert.deepEqual(await pages(ctx, '/api/v1/admin/sandboxes?owner=subject', {expected: ['one', 'two']}), ['one', 'two']);
  assert.deepEqual(seen.map(x => x.am), ['a', 'b']);
  assert.match(seen[1].path, /owner=subject/); assert.match(seen[1].path, /cursor=cursor/);
  let calls = 0;
  ctx.request = async arg => {calls++; return {timing: {am: arg.am, request_id: 'test-request'}, body: {items: [{id: 'duplicate'}], next: 'same'}};};
  await assert.rejects(pages(ctx, '/api/v1/admin/sandboxes'), /duplicate/); assert.equal(calls, 2);
});
test('fixture cleanup custody is registered before invalid scope is rejected', async () => {
  let cleanup;
  const ctx = {fixture: () => ({take: async () => ({scope: 'wrong', cleanup: async () => {}})}),
    defer: (_label, fn) => cleanup = fn};
  await assert.rejects(take(ctx, 'usage'), /admin_setup_not_disposable/);
  assert.equal(typeof cleanup, 'function');
});
test('fixture requirements are concrete setup contracts, not pass-result delegates', () => {
  const f = JSON.parse(readFileSync(new URL('./fixtures.json', import.meta.url)));
  assert.equal(f.issue, 2149); assert.ok(f['admin-fixtures'].usage.requirements.length >= 4);
  assert.ok(!JSON.stringify(f).includes('runCase'));
});

test('same-key uncertain pool retry rejects changed body, key, result and duplicate sends', async () => {
  const {sameIntent} = await import('./warm-pools.mjs');
  const accepted = {method: 'POST', status: 202, idempotency_sha256: 'a'.repeat(64), request_body: {cpu_millis: 250, memory_mib: 256, target_replicas: 1}, body: {id: 'pool-one'}};
  assert.equal(sameIntent([accepted, structuredClone(accepted)]), true);
  for (const mutate of [r => r.idempotency_sha256 = 'b'.repeat(64), r => r.request_body.target_replicas = 2,
    r => r.body.id = 'different', r => r.status = 200, r => r.method = 'PUT']) {
    const changed = structuredClone(accepted); mutate(changed); assert.equal(sameIntent([accepted, changed]), false);
  }
  assert.equal(sameIntent([accepted, accepted, accepted]), false);
});
test('page inspector rejects cross-report continuation before accepting its rows', async () => {
  const ctx = {am: 'a', assert: (value, code) => assert.ok(value, code),
    request: async arg => ({timing: {am: arg.am, request_id: 'test-request'}, body: {items: [{subject: 'one'}], report_id: 'other', next: null}})};
  await assert.rejects(pages(ctx, '/reports/expected/owners', {key: 'subject', inspect: body =>
    ctx.assert(body.report_id === 'expected', 'owner_page_wrong_report')}), /owner_page_wrong_report/);
});

test('selected descriptor must match current URL filters and the loaded inventory identity', async () => {
  const {matchesSelection, selectedReport} = await import('./helpers.mjs');
  const current = report(); current.query.filters = {owner: 'owner-one', statuses: ['active'], sandbox_id: null,
    runtime_version: 'image:one', agent_id: null, search: ''};
  const params = new URLSearchParams({owner: 'owner-one', status: 'active', runtime: 'image:one'});
  assert.equal(matchesSelection(current, '24h', params), true);
  const rows = [{body: current}, {status: 200, path: '/reports/report-1/sandboxes?limit=50', body: {report_id: 'report-1', items: []}}];
  assert.equal(selectedReport(rows, '24h', params), current);
  for (const [key, value] of [['owner', 'other'], ['sandbox', 'selected'], ['agent', 'agent'], ['status', 'paused'], ['search', 'legacy']]) {
    const wrong = new URLSearchParams(params); wrong.set(key, value);
    assert.equal(selectedReport(rows, '24h', wrong), undefined);
  }
  assert.equal(selectedReport(rows, '7d', params), undefined);
  assert.equal(selectedReport([...rows, {status: 200, path: '/reports/other/sandboxes', body: {report_id: 'other'}}], '24h', params), undefined);
  assert.equal(selectedReport([{body: current}], '24h', params), undefined);
});
test('backoff evidence refuses rapid retries and absent timing', async () => {
  const {backoffObserved} = await import('./usage-faults.mjs');
  assert.equal(backoffObserved([{status: 429, observed_ms: 0}, {status: 503, observed_ms: 2000}], 1900), true);
  assert.equal(backoffObserved([{status: 429, observed_ms: 0}, {status: 503, observed_ms: 500}], 1900), false);
  assert.equal(backoffObserved([{status: 429}], 1900), false);
  assert.equal(backoffObserved([], 1900), false);
});
test('complete assigned inventory is present exactly once', () => {
  const assignment = JSON.parse(readFileSync(new URL('../../assignments/developer-09.json', import.meta.url)));
  const expectedIds = assignment.case_ids ?? assignment.cases;
  assert.equal(cases.length, 37);
  assert.equal(new Set(cases.map(c => c.id)).size, 37);
  if (expectedIds) assert.deepEqual(cases.map(c => c.id).sort(), [...expectedIds].sort());
});
test('browser cleanup closes pending transport before collecting receipts and preserves an early block', async () => {
  const {open} = await import('./helpers.mjs'); const order = [], deferred = [];
  const session = {page: {}, close: async () => {order.push('close');}};
  const browser = {open: async () => session, evidence: async () => {
    assert.equal(order[0], 'close'); order.push('evidence'); return {requests: []};}};
  const ctx = {am: 'a', fixture: () => browser, defer: (_label, operation) => deferred.push(operation),
    observe: async () => assert.fail('an empty browser receipt cannot become a PASS observation')};
  assert.equal(await open(ctx), session); await deferred[0]();
  assert.deepEqual(order, ['close', 'evidence']);
});
test('browser alias and canonical API paths bind the same exact usage route', async () => {
  const {canonicalPath} = await import('./helpers.mjs');
  assert.equal(canonicalPath('http://localhost/api/agent-manager/v1/admin/usage/metrics/reports/view?window=24h'), '/api/v1/admin/usage/metrics/reports/view');
  assert.equal(canonicalPath('/api/v1/admin/usage/metrics/reports/view'), '/api/v1/admin/usage/metrics/reports/view');
  assert.equal(canonicalPath('/api/agent-manager-invalid/v1/agents'), '/api/agent-manager-invalid/v1/agents');
});
