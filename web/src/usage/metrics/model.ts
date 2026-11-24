import type {
  Cma,
  UsageMetricQuery,
  UsageMetricQuantity,
  UsageMetricSort,
  UsageMetricStatus,
} from '@cma/client';
import type {
  UsageInitialCreate,
  UsageInitialData,
  UsageInitialProjection,
  UsageInitialReport,
  UsageInitialView,
  UsageInitialViewInput,
} from '@cma/client';
import { statusLabel } from '../../vocabulary/status';

type LegacyMetricsClient = Pick<
  Cma['usageMetrics'],
  | 'create'
  | 'report'
  | 'sandboxes'
  | 'sandbox'
  | 'series'
  | 'owners'
  | 'options'
  | 'ownerAllocations'
>;
type InitialMetricsClient = Partial<
  Pick<Cma['usageMetrics'], 'createInitial' | 'initialReport' | 'latestInitial'>
>;
export type MetricsClient = LegacyMetricsClient & InitialMetricsClient;
export type {
  UsageInitialCreate,
  UsageInitialData,
  UsageInitialProjection,
  UsageInitialReport,
  UsageInitialView,
  UsageInitialViewInput,
};
export type Metric = 'cpu' | 'memory' | 'disk';
export type Group = 'total' | 'owner' | 'sandbox';
export type Range = '1h' | '24h' | '7d' | '30d' | 'custom';
export interface Selection {
  readonly range: Range;
  readonly from: string;
  readonly to: string;
  readonly owner: string;
  readonly status: UsageMetricStatus | '';
  readonly runtime: string;
  readonly search: string;
  readonly sandbox: string;
  readonly agent: string;
  readonly sort: UsageMetricSort;
  readonly order: 'asc' | 'desc';
}
export const sorts: readonly UsageMetricSort[] = [
  'name',
  'owner',
  'status',
  'last_activity_at',
  'cpu_mean_cores',
  'memory_mean_bytes',
  'disk_latest_bytes',
  'allocated_cpu_core_hours',
  'allocated_memory_gib_hours',
  'allocated_disk_gib_hours',
];
export const statuses: readonly UsageMetricStatus[] = [
  'provisioning',
  'installing',
  'active',
  'pausing',
  'paused',
  'reviving',
  'credential_rotating',
  'rotating',
  'failed',
  'retiring',
  'retired',
];
export const statusOptions = statuses.map((value) => ({ value, label: statusLabel(value) }));
export const defaultSelection: Selection = {
  range: '24h',
  from: '',
  to: '',
  owner: '',
  status: '',
  runtime: '',
  search: '',
  sandbox: '',
  agent: '',
  sort: 'last_activity_at',
  order: 'desc',
};

export function metricQuery(
  selection: Selection,
  admin: boolean,
  now = Date.now(),
): UsageMetricQuery {
  const durations = { '1h': 3600, '24h': 86400, '7d': 604800, '30d': 2592000 };
  // Align presets so the seven-day range fits the API's 2,016 bucket ceiling.
  const step = selection.range === '7d' ? 300 : selection.range === '30d' ? 3600 : 60;
  const to =
    selection.range === 'custom'
      ? Math.floor(Date.parse(selection.to) / 1000)
      : Math.floor(now / 1000 / step) * step;
  const from =
    selection.range === 'custom'
      ? Math.floor(Date.parse(selection.from) / 1000)
      : to - durations[selection.range];
  if (
    !Number.isFinite(from) ||
    !Number.isFinite(to) ||
    from < 0 ||
    from >= to ||
    to - from > 2592000 ||
    to > now / 1000
  ) {
    throw new Error('Choose a valid range of up to 30 days, ending no later than now.');
  }
  return {
    from,
    to,
    // Let the service choose custom-range buckets, including boundary buckets.
    ...(selection.range === 'custom' ? {} : { step_seconds: step }),
    filters: {
      ...(admin && selection.owner ? { owner: selection.owner } : {}),
      statuses: selection.status ? [selection.status] : [],
      runtime_version: selection.runtime || undefined,
      sandbox_id: selection.sandbox || undefined,
      agent_id: selection.agent || undefined,
      search: selection.search,
    },
  };
}

/** Conversion is only for display/plotting. Exact large integers remain in the SDK. */
export function displayQuantity(
  value: UsageMetricQuantity | null | undefined,
  divisor = 1,
): string {
  if (value == null) return 'Unavailable';
  if (typeof value === 'string' && divisor === 1) return BigInt(value).toLocaleString('en-SG');
  return new Intl.NumberFormat('en-SG', { maximumFractionDigits: 3 }).format(
    Number(value) / divisor,
  );
}
export function metricValue(value: UsageMetricQuantity | null | undefined, metric: Metric): string {
  return value == null
    ? 'Unavailable'
    : `${displayQuantity(value, metric === 'cpu' ? 1 : 2 ** 30)} ${metric === 'cpu' ? 'cores' : 'GiB'}`;
}
export function metricTime(value: UsageMetricQuantity | null | undefined): string {
  if (value == null) return 'Not observed';
  const date = new Date(Number(value) * 1000);
  return Number.isNaN(date.getTime())
    ? 'Time unavailable'
    : `${date.toLocaleString('en-SG', { timeZone: 'Asia/Singapore' })} SGT`;
}
export const metricLabels = {
  cpu: 'CPU',
  memory: 'Memory working set',
  disk: 'Home PVC disk',
} as const;
