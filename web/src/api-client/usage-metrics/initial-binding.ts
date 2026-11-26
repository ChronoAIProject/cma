import { ProtocolError } from '../errors.js';
import type { Parser } from '../schema.js';
import type { UsageMetricQuery } from './index.js';
import type { NormalizedProjection, UsageInitialData } from './initial.js';
import type { UsageMetricReport } from './schema.js';

function requireMatch(ok: boolean, reason: string): asserts ok {
  if (!ok) throw new ProtocolError(`initial usage ${reason}`);
}
export function strict<T>(parse: Parser<T>, raw: unknown): T {
  const parsed = parse(raw);
  const check = (source: unknown, output: unknown) => {
    if (Array.isArray(source) && Array.isArray(output)) {
      source.forEach((value, index) => check(value, output[index]));
    } else if (
      source !== null &&
      typeof source === 'object' &&
      output !== null &&
      typeof output === 'object'
    ) {
      for (const key of Object.keys(source)) {
        requireMatch(Object.hasOwn(output, key), 'response contains an unknown field');
        check((source as Record<string, unknown>)[key], (output as Record<string, unknown>)[key]);
      }
    }
  };
  check(raw, parsed);
  return parsed;
}
export function bindReport(report: UsageMetricReport, status: number, admin: boolean) {
  requireMatch(
    report.schema_version === 1 &&
      /^umr_[a-f0-9]{32}$/.test(report.report_id) &&
      report.scope.admin === admin &&
      report.scope.principal.length > 0,
    'descriptor does not match scope',
  );
  const pending = report.status === 'pending';
  requireMatch(
    (status === 202 && pending && !report.report_complete && report.summary === null) ||
      (status === 200 &&
        report.status === 'ready' &&
        report.report_complete &&
        report.summary !== null),
    'completion state does not match HTTP status',
  );
  const from = BigInt(report.query.from),
    to = BigInt(report.query.to),
    step = BigInt(report.query.step_seconds),
    at = BigInt(report.as_of);
  requireMatch(
    from < to && to <= at && to - from <= 2592000n && step > 0n && step <= 86400n,
    'range is invalid',
  );
  requireMatch((to - 1n) / step - from / step < 2016n, 'point limit exceeded');
  const cutoff = at > 120n ? at - 120n : 0n;
  const effective = to < cutoff ? to : cutoff;
  requireMatch(
    BigInt(report.effective_range.from) === from &&
      BigInt(report.effective_range.to) === (effective > from ? effective : from),
    'effective range does not match',
  );
  requireMatch(
    BigInt(report.expires_at) > at &&
      BigInt(report.expires_at) > BigInt(Math.floor(Date.now() / 1000)),
    'descriptor is expired',
  );
  if (report.query.filters.agent_id !== null)
    requireMatch(
      report.query.filters.sandbox_id !== null &&
        /^sbx_[a-f0-9]{32}$/.test(report.query.filters.sandbox_id),
      'selected Agent has no bound parent',
    );
}
export function bindQuery(report: UsageMetricReport, input: UsageMetricQuery) {
  for (const key of ['from', 'to', 'step_seconds'] as const)
    if (input[key] !== undefined)
      requireMatch(BigInt(report.query[key]) === BigInt(input[key]), 'query does not match');
  const wanted = input.filters ?? {},
    actual = report.query.filters;
  requireMatch(
    (wanted.owner ?? null) === actual.owner &&
      (wanted.runtime_version ?? null) === actual.runtime_version &&
      (wanted.agent_id ?? null) === actual.agent_id &&
      (wanted.sandbox_id == null || wanted.sandbox_id === actual.sandbox_id) &&
      (wanted.agent_id != null || wanted.sandbox_id != null || actual.sandbox_id === null) &&
      (wanted.search ?? '').trim().toLowerCase() === actual.search &&
      [...new Set(wanted.statuses ?? [])].sort().join(',') ===
        [...actual.statuses].sort().join(','),
    'filters do not match',
  );
}
export function bindData(
  report: UsageMetricReport,
  data: UsageInitialData | undefined,
  projection: NormalizedProjection,
) {
  if (report.status === 'pending') {
    requireMatch(data === undefined, 'pending response contains children');
    return;
  }
  requireMatch(data !== undefined && report.summary !== null, 'ready response is incomplete');
  requireMatch(
    JSON.stringify(data.projection) === JSON.stringify(projection),
    'projection does not match',
  );
  const rows = data.rows,
    count = BigInt(report.summary.sandbox_count),
    limit = BigInt(projection.first_page.limit);
  requireMatch(
    rows.report_id === report.report_id &&
      BigInt(rows.as_of) === BigInt(report.as_of) &&
      BigInt(rows.total_count) === count,
    'page identity does not match',
  );
  requireMatch(
    BigInt(rows.items.length) === (count < limit ? count : limit) &&
      (rows.next !== null) === count > limit &&
      (rows.next === null || (rows.next.length > 0 && rows.next.length <= 4096)) &&
      new Set(rows.items.map((row) => row.sandbox_id)).size === rows.items.length,
    'page membership is incomplete',
  );
  for (const row of rows.items)
    requireMatch(
      /^sbx_[a-f0-9]{32}$/.test(row.sandbox_id) &&
        (report.scope.admin || row.owner.subject === report.scope.principal) &&
        (report.query.filters.owner === null || row.owner.subject === report.query.filters.owner) &&
        (report.query.filters.sandbox_id === null ||
          row.sandbox_id === report.query.filters.sandbox_id),
      'row scope does not match',
    );
  requireMatch(
    data.charts.length === projection.total_metrics.length,
    'chart selection is incomplete',
  );
  const from = BigInt(report.effective_range.from),
    to = BigInt(report.effective_range.to),
    step = BigInt(report.query.step_seconds);
  const points = to === from ? 0n : (to - 1n) / step - from / step + 1n;
  for (const [index, chart] of data.charts.entries()) {
    requireMatch(
      chart.report_id === report.report_id &&
        chart.metric === projection.total_metrics[index] &&
        chart.unit === (chart.metric === 'cpu' ? 'cores' : 'bytes') &&
        chart.source === 'victoriametrics:kubelet-resource-and-volume:v1' &&
        chart.lines.length === (count === 0n ? 0 : 1),
      'chart identity does not match',
    );
    for (const line of chart.lines) {
      requireMatch(
        line.id === 'total' && BigInt(line.points.length) === points && points <= 2016n,
        'total chart is incomplete',
      );
      for (const [i, point] of line.points.entries()) {
        const start = (from / step + BigInt(i)) * step,
          end = start + step;
        requireMatch(
          BigInt(point.from) === (start > from ? start : from) &&
            BigInt(point.to) === (end < to ? end : to),
          'chart bucket does not match',
        );
      }
    }
  }
}
