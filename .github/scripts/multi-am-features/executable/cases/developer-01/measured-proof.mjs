import {check, required, equal, id} from './common.mjs';

export const quantity = value => (typeof value === 'number' && Number.isFinite(value) && value >= 0
  && (!Number.isInteger(value) || Number.isSafeInteger(value)))
  || (typeof value === 'string' && /^[1-9][0-9]{15,308}$/.test(value) && BigInt(value) > BigInt(Number.MAX_SAFE_INTEGER));
export const displayQuantity = (value, divisor = 1) => {
  check(quantity(value), 'admin_measured_quantity');
  return typeof value === 'string' && divisor === 1 ? BigInt(value).toLocaleString('en-SG')
    : new Intl.NumberFormat('en-SG', {maximumFractionDigits: 3}).format(Number(value) / divisor);
};
export const metricTime = value => new Date(Number(value) * 1000).toLocaleString('en-SG', {timeZone: 'Asia/Singapore'}) + ' SGT';
export const allocationKeys = ['cpu_core_hours', 'memory_gib_hours', 'disk_gib_hours'];
export const capacityKeys = ['cpu_cores', 'memory_bytes', 'disk_bytes'];

export function measuredFixture(f) {
  required(Number.isSafeInteger(f.range?.from) && Number.isSafeInteger(f.range?.to) && f.range.from >= 0
    && f.range.to > f.range.from && f.range.to - f.range.from <= 2592000 && f.range.to <= Date.now() / 1000
    && ['user', 'admin'].every(actor => typeof f.scopes?.[actor]?.subject === 'string' && f.scopes[actor].subject.trim()
      && Array.isArray(f.scopes[actor].sandboxIds) && f.scopes[actor].sandboxIds.length > 0
      && f.scopes[actor].sandboxIds.length <= 50
      && new Set(f.scopes[actor].sandboxIds).size === f.scopes[actor].sandboxIds.length)
    && f.scopes.user.subject !== f.scopes.admin.subject, 'admin_measured_scope_fixture');
  for (const scope of Object.values(f.scopes)) for (const sandbox of scope.sandboxIds) id(sandbox);
}

export function reportProof(report, actor, f) {
  const scope = f.scopes[actor];
  check(report?.status === 'ready' && report.report_complete === true && typeof report.report_id === 'string'
    && report.report_id.trim() && report.scope?.principal === scope.subject && report.scope.admin === (actor === 'admin'),
  'admin_measured_report_scope');
  check(report.query?.from === f.range.from && report.query.to === f.range.to
    && ['owner', 'sandbox_id', 'agent_id', 'runtime_version'].every(k => report.query.filters?.[k] === null)
    && equal(report.query.filters.statuses, []) && report.query.filters.search === '', 'admin_measured_report_range');
  check(report.summary?.sandbox_count === scope.sandboxIds.length
    && allocationKeys.every(k => quantity(report.summary.allocated_period?.[k]))
    && capacityKeys.every(k => quantity(report.summary.total_allocated_capacity?.[k])), 'admin_measured_allocation_shape');
  return report;
}

export function selectedMeasured(rows, actor, f, base) {
  const inventories = rows.filter(r => r.status === 200 && /^\/[^/]+\/sandboxes$/.test(r.path.slice(base.length)));
  const inventory = inventories.at(-1);
  if (!inventory) return;
  check(new Set(inventories.map(r => r.body?.report_id)).size === 1, 'admin_measured_ambiguous_report');
  const descriptor = rows.filter(r => r.status === 200).map(r => r.body?.report ?? r.body)
    .filter(r => r?.report_id === inventory.body?.report_id && r.status === 'ready').at(-1);
  if (!descriptor) return;
  reportProof(descriptor, actor, f);
  check(inventory.path === base + '/' + id(descriptor.report_id) + '/sandboxes'
    && inventory.body.next === null && inventory.body.total_count === f.scopes[actor].sandboxIds.length
    && Array.isArray(inventory.body.items)
    && equal(inventory.body.items.map(r => r.sandbox_id).toSorted(), f.scopes[actor].sandboxIds.toSorted()),
  'admin_measured_report_inventory');
  return {report: descriptor, inventory: inventory.body};
}

export function measuredSeries(rows, reportId, base, metric) {
  const row = rows.filter(r => r.status === 200 && r.path === base + '/' + id(reportId) + '/series'
    && new URLSearchParams(r.query).get('metric') === metric && new URLSearchParams(r.query).get('group_by') === 'total').at(-1);
  if (!row) return;
  const series = row.body;
  check(series.report_id === reportId && series.metric === metric && series.unit === (metric === 'cpu' ? 'cores' : 'bytes')
    && typeof series.source === 'string' && Array.isArray(series.lines), 'admin_measured_series_identity');
  return series;
}

export function cpuGap(rows, reportId, base) {
  const series = measuredSeries(rows, reportId, base, 'cpu');
  if (!series) return;
  // Default total grouping has one series. Bind the exact null bucket to its
  // accessible table row, instead of accepting a generic explanatory caption.
  check(series.lines.length === 1 && series.lines[0].id === 'total'
    && Array.isArray(series.lines[0].points) && series.lines[0].points.length <= 2016, 'admin_measured_total_series');
  const index = series.lines[0].points.findIndex(p => p.mean === null && typeof p.reason === 'string' && p.reason.trim());
  required(index >= 0, 'admin_measured_real_gap_required');
  const point = series.lines[0].points[index];
  check(Number.isSafeInteger(point.from) && Number.isSafeInteger(point.to) && point.from < point.to,
    'admin_measured_gap_interval');
  return {point, index, source: series.source};
}
