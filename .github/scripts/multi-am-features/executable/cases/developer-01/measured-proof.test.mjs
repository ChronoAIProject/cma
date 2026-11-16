import test from 'node:test';
import assert from 'node:assert/strict';
import {quantity, displayQuantity, measuredFixture, reportProof, selectedMeasured, cpuGap} from './measured-proof.mjs';
import {measuredModel} from './measured-model.mjs';
import {waitMeasured} from './measured-report.mjs';

for (const value of [null, undefined, '', '1', '-1', '1.2', -1, Infinity, NaN, 9007199254740992])
  test('invalid quantity stays unavailable to qualification: ' + String(value), () => {
    assert.equal(quantity(value), false); assert.throws(() => displayQuantity(value), /admin_measured_quantity/);
  });
test('large exact allocation integers retain precision in displayed totals', () => {
  assert.equal(quantity('9007199254740993'), true);
  assert.equal(displayQuantity('9007199254740993'), '9,007,199,254,740,993');
  assert.equal(displayQuantity(2 ** 30, 2 ** 30), '1');
  assert.equal(displayQuantity(1.23456), '1.235');
});
for (const mutation of [f => delete f.range, f => f.range.from = -1, f => f.range.to = f.range.from,
  f => f.range.to = f.range.from + 2592001, f => f.scopes.user.subject = '',
  f => f.scopes.admin.subject = f.scopes.user.subject, f => f.scopes.user.sandboxIds = [],
  f => f.scopes.user.sandboxIds.push(f.scopes.user.sandboxIds[0]), f => f.scopes.user.sandboxIds = ['bad/id']]) {
  test('invalid range or scope setup refuses before browser use', () => {
    const h = measuredModel(); mutation(h.f); assert.throws(() => measuredFixture(h.f));
    assert.equal(h.sessions.length, 0);
  });
}
const f = measuredModel().f;
const base = '/api/v1/usage/metrics/reports';
const valid = () => ({report_id: 'one', status: 'ready', report_complete: true,
  scope: {admin: false, principal: f.scopes.user.subject}, query: {...f.range, filters: {
    owner: null, sandbox_id: null, agent_id: null, runtime_version: null, statuses: [], search: ''}},
  summary: {sandbox_count: 1, allocated_period: {cpu_core_hours: 1, memory_gib_hours: 2, disk_gib_hours: 3},
    total_allocated_capacity: {cpu_cores: 2, memory_bytes: 4, disk_bytes: 8}}});
for (const mutate of [r => r.scope.admin = true, r => r.query.filters.owner = 'foreign',
  r => r.summary.sandbox_count = 0, r => r.summary.allocated_period.cpu_core_hours = null,
  r => r.summary.total_allocated_capacity.disk_bytes = '2', r => r.report_id = '']) {
  test('report proof rejects wrong scope and missing allocations', () => {
    const report = valid(); mutate(report); assert.throws(() => reportProof(report, 'user', f));
  });
}
test('unrelated ready report cannot replace the inventory-selected report', () => {
  const report = valid(), inventory = {status: 200, path: base + '/one/sandboxes',
    body: {report_id: 'one', next: null, total_count: 1, items: [{sandbox_id: 'sandbox-user'}]}};
  const rows = [{status: 200, path: base + '/view', body: report}, inventory,
    {status: 200, path: base + '/view', body: {...report, report_id: 'background'}}];
  assert.equal(selectedMeasured(rows, 'user', f, base).report.report_id, 'one');
  assert.equal(selectedMeasured([rows[2], inventory], 'user', f, base), undefined);
});
for (const mutate of [s => s.metric = 'disk', s => s.unit = 'bytes', s => s.lines = [],
  s => s.lines[0].id = 'foreign', s => s.lines[0].points[0].from = 5,
  s => s.lines[0].points[0].to = 0, s => s.lines[0].points[0].reason = null]) {
  test('chart gap must be a source-bound exact total-series interval', () => {
    const series = {report_id: 'one', metric: 'cpu', unit: 'cores', source: 'native',
      lines: [{id: 'total', points: [{from: 1, to: 2, mean: null, reason: 'no_measurement'}]}]};
    mutate(series);
    assert.throws(() => cpuGap([{status: 200, path: base + '/one/series', query: '?metric=cpu&group_by=total', body: series}], 'one', base));
  });
}
test('a cancelled pending observation issues no subsequent read', async () => {
  const h = measuredModel(); h.controller.abort(Error('cancelled')); let reads = 0;
  await assert.rejects(waitMeasured(h.ctx, async () => {reads++;}, 'not_ready'), /cancelled/);
  assert.equal(reads, 0);
});
test('wait joins a pending response on its next observation', async () => {
  const h = measuredModel(); let reads = 0;
  assert.equal(await waitMeasured(h.ctx, async () => ++reads > 1 ? 'ready' : undefined, 'not_ready'), 'ready');
  assert.equal(reads, 2);
});

test('competing inventory report identities cannot be assigned to one committed DOM', () => {
  const descriptor = {status: 200, path: base + '/view', body: valid()};
  const inventory = {status: 200, path: base + '/one/sandboxes', body: {report_id: 'one', next: null,
    total_count: 1, items: [{sandbox_id: 'sandbox-user'}]}};
  assert.throws(() => selectedMeasured([descriptor, inventory,
    {...inventory, path: base + '/other/sandboxes', body: {...inventory.body, report_id: 'other'}}], 'user', f, base),
  /admin_measured_ambiguous_report/);
});
