import type { ResolvedConfig } from '../config.js';
import { ApiError, ProtocolError, ValidationError } from '../errors.js';
import { request, type OperationOptions } from '../http.js';
import { segment } from '../resources.js';
import { array, object, optional, type Output } from '../schema.js';
import type { UsageMetricQuery, UsageMetricSort } from './index.js';
import { page, report, series } from './schema.js';
import { parseView, type UsageMetricView, type UsageMetricViewQuery } from './view.js';
import { bindData, bindQuery, bindReport, strict } from './initial-binding.js';

type Metric = 'cpu' | 'memory' | 'disk';
export interface UsageInitialProjection {
  readonly first_page?: {
    readonly limit?: number;
    readonly sort?: UsageMetricSort;
    readonly order?: 'asc' | 'desc';
  };
  readonly total_metrics?: readonly Metric[];
}
export interface UsageInitialCreate {
  readonly query: UsageMetricQuery;
  readonly projection: UsageInitialProjection;
}
export interface UsageInitialViewInput {
  readonly selection: UsageMetricViewQuery;
  readonly projection: UsageInitialProjection;
}
export interface NormalizedProjection {
  readonly first_page: {
    readonly limit: number;
    readonly sort: UsageMetricSort;
    readonly order: 'asc' | 'desc';
  };
  readonly total_metrics: readonly Metric[];
}
const sorts: readonly string[] = [
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
function keys(value: object, allowed: readonly string[]) {
  if (
    value === null ||
    typeof value !== 'object' ||
    Array.isArray(value) ||
    Object.keys(value).some((key) => !allowed.includes(key))
  )
    throw new ValidationError('unsupported initial projection fields');
}
export function normalizeProjection(
  input: UsageInitialProjection,
  admin: boolean,
): NormalizedProjection {
  keys(input, ['first_page', 'total_metrics']);
  if (input.first_page !== undefined) keys(input.first_page, ['limit', 'sort', 'order']);
  const limit = input.first_page?.limit ?? 50;
  const sort = input.first_page?.sort ?? 'last_activity_at';
  const order = input.first_page?.order ?? 'desc';
  const metrics = input.total_metrics ?? [];
  if (
    !Number.isInteger(limit) ||
    limit < 1 ||
    limit > 50 ||
    !sorts.includes(sort) ||
    !['asc', 'desc'].includes(order) ||
    (!admin && sort === 'owner') ||
    !Array.isArray(metrics) ||
    metrics.length > 3 ||
    new Set(metrics).size !== metrics.length ||
    metrics.some((metric) => !['cpu', 'memory', 'disk'].includes(metric))
  )
    throw new ValidationError(
      'initial views allow 1–50 rows and up to three distinct total metrics',
    );
  return { first_page: { limit, sort, order }, total_metrics: [...metrics] };
}
const data = object({
  projection: (value: unknown) => {
    // A returned projection must contain every normalized field.
    const result = value as NormalizedProjection;
    if (
      !result?.first_page ||
      result.first_page.limit === undefined ||
      result.first_page.sort === undefined ||
      result.first_page.order === undefined ||
      result.total_metrics === undefined
    )
      throw new ProtocolError('incomplete initial projection');
    try {
      return normalizeProjection(result, true);
    } catch (cause) {
      throw new ProtocolError('invalid initial projection', { cause });
    }
  },
  rows: page,
  charts: array(series),
});
const initialReport = object({ report, initial: optional(data) });
export type UsageInitialData = Output<typeof data>;
export type UsageInitialReport = Output<typeof initialReport>;
export interface UsageInitialView {
  readonly view: UsageMetricView;
  readonly initial?: UsageInitialData;
}
const REST_ROUTE_NOT_FOUND = 'https://spec.chrono-ai.fun/cma/1.0/problems/not-found';

/** Only an explicitly missing initial route permits the legacy report path. */
export function isUsageInitialUnavailable(error: unknown): boolean {
  return (
    error instanceof ApiError &&
    ((error.status === 501 && error.code === 'report_view_disabled') ||
      (error.status === 404 &&
        Object.keys(error.extensions).length === 0 &&
        (error.code === undefined ||
          (error.code === 'not_found' && error.problemType === REST_ROUTE_NOT_FOUND))))
  );
}
const base = (admin: boolean) => `/api/v1/${admin ? 'admin/' : ''}usage/metrics/initial`;

async function send(
  config: ResolvedConfig,
  path: string,
  body: unknown,
  options: OperationOptions,
) {
  let status = 0;
  const value = await request(
    {
      ...config,
      responseObserver: async (metadata) => {
        status = metadata.status;
        await config.responseObserver?.(metadata);
      },
    },
    path,
    { ...options, method: 'POST', body, expectedStatus: [200, 202] },
  );
  return { status, value };
}
function parseReport(
  value: unknown,
  status: number,
  admin: boolean,
  projection: NormalizedProjection,
): UsageInitialReport {
  const parsed = strict(initialReport, value);
  bindReport(parsed.report, status, admin);
  bindData(parsed.report, parsed.initial, projection);
  return parsed;
}
export async function createInitial(
  config: ResolvedConfig,
  admin: boolean,
  input: UsageInitialCreate,
  options: OperationOptions,
): Promise<UsageInitialReport> {
  const projection = normalizeProjection(input.projection, admin);
  const captured = structuredClone(input);
  if (!admin && captured.query.filters?.owner != null)
    throw new ValidationError('owner filters require administrator reporting');
  const response = await send(config, `${base(admin)}/reports`, captured, options);
  const result = parseReport(response.value, response.status, admin, projection);
  bindQuery(result.report, captured.query);
  return result;
}
export async function readInitial(
  config: ResolvedConfig,
  admin: boolean,
  id: string,
  input: UsageInitialProjection,
  options: OperationOptions,
): Promise<UsageInitialReport> {
  const projection = normalizeProjection(input, admin);
  const response = await send(
    config,
    `${base(admin)}/reports/${segment(id)}`,
    structuredClone(input),
    options,
  );
  const result = parseReport(response.value, response.status, admin, projection);
  if (result.report.report_id !== id)
    throw new ProtocolError('initial report identity did not match');
  return result;
}
export async function latestInitial(
  config: ResolvedConfig,
  admin: boolean,
  input: UsageInitialViewInput,
  options: OperationOptions,
): Promise<UsageInitialView> {
  const projection = normalizeProjection(input.projection, admin);
  const captured = structuredClone(input);
  if (!admin && captured.selection.filters?.owner != null)
    throw new ValidationError('owner filters require administrator reporting');
  const response = await send(config, `${base(admin)}/view`, captured, options);
  const parse = object({
    view: (value: unknown) => parseView(value, admin, captured.selection),
    initial: optional(data),
  });
  const result = strict(parse, response.value);
  bindReport(result.view.report, response.status, admin);
  const selected = result.view;
  const rangeFrom = BigInt(selected.requested_range.from),
    rangeTo = BigInt(selected.requested_range.to);
  const seconds =
    selected.window === '24h'
      ? 86400n
      : selected.window === '7d'
        ? 604800n
        : selected.window === '30d'
          ? 2592000n
          : undefined;
  if (
    rangeFrom >= rangeTo ||
    (seconds === undefined &&
      (rangeFrom !== BigInt(selected.report.query.from) ||
        rangeTo !== BigInt(selected.report.query.to))) ||
    (seconds !== undefined && rangeTo - rangeFrom !== seconds) ||
    (selected.generated_at !== null &&
      (BigInt(selected.generated_at) < BigInt(selected.report.as_of) ||
        BigInt(selected.generated_at) >= BigInt(selected.report.expires_at)))
  )
    throw new ProtocolError('initial view metadata does not match its selection');

  bindData(result.view.report, result.initial, projection);
  return result;
}
