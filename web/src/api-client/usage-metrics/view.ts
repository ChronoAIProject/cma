import type { ResolvedConfig } from '../config.js';
import { ApiError, ProtocolError, ValidationError } from '../errors.js';
import type { OperationOptions } from '../http.js';
import { query, read } from '../resources.js';
import { boolean, enumeration, nullable, object, text, type Output } from '../schema.js';
import type { UsageMetricQuery } from './index.js';
import { integer, range, report } from './schema.js';

export interface UsageMetricViewQuery extends UsageMetricQuery {
  readonly window?: '24h' | '7d' | '30d' | 'custom';
  readonly refresh?: boolean;
}
const viewSchema = object({
  schema_version: integer,
  window: enumeration('24h', '7d', '30d', 'custom'),
  requested_range: range,
  report,
  generated_at: nullable(integer),
  stale: boolean,
  refresh: object({
    state: enumeration('idle', 'pending', 'deferred'),
    report_id: nullable(text),
    retry_after_seconds: integer,
  }),
});
export type UsageMetricView = Output<typeof viewSchema>;

/** Only explicit feature/route absence permits the old authorized workflow. */
export function isUsageViewUnavailable(error: unknown): boolean {
  return (
    error instanceof ApiError &&
    ((error.status === 501 && error.code === 'report_view_disabled') ||
      (error.status === 404 &&
        error.code === undefined &&
        Object.keys(error.extensions).length === 0))
  );
}
export function latestView(
  config: ResolvedConfig,
  admin: boolean,
  input: UsageMetricViewQuery,
  options: OperationOptions,
): Promise<UsageMetricView> {
  const window = input.window ?? '24h';
  if (!admin && input.filters?.owner != null)
    throw new ValidationError('owner filters require administrator reporting');
  if (
    (window === 'custom' && (input.from === undefined || input.to === undefined)) ||
    (window !== 'custom' && (input.from !== undefined || input.to !== undefined))
  )
    throw new ValidationError('only custom windows accept and require from/to');
  return read(
    config,
    query(`/api/v1/${admin ? 'admin/' : ''}usage/metrics/view`, {
      window,
      from: input.from,
      to: input.to,
      step_seconds: input.step_seconds,
      filters: input.filters === undefined ? undefined : JSON.stringify(input.filters),
      refresh: input.refresh?.toString(),
    }),
    (value) => parseView(value, admin, input),
    { ...options, expectedStatus: [200, 202] },
  );
}

export function parseView(
  value: unknown,
  admin: boolean,
  input: UsageMetricViewQuery,
): UsageMetricView {
  const window = input.window ?? '24h';

  const view = viewSchema(value);
  const snapshot = view.report;
  const pending = snapshot.status === 'pending';
  const ready = snapshot.status === 'ready';
  if (
    view.schema_version !== 1 ||
    view.window !== window ||
    snapshot.scope.admin !== admin ||
    (!pending && !ready) ||
    (pending &&
      (view.generated_at !== null ||
        view.stale ||
        view.refresh.state !== 'pending' ||
        view.refresh.report_id !== snapshot.report_id)) ||
    (ready &&
      (view.generated_at === null || !snapshot.report_complete || snapshot.summary === null)) ||
    (input.step_seconds !== undefined &&
      BigInt(snapshot.query.step_seconds) !== BigInt(input.step_seconds)) ||
    (window === 'custom' &&
      (BigInt(snapshot.query.from) !== BigInt(input.from!) ||
        BigInt(snapshot.query.to) !== BigInt(input.to!)))
  )
    throw new ProtocolError(
      'usage view does not match its requested selection or completion state',
    );
  const wanted = input.filters ?? {};
  const actual = snapshot.query.filters;
  const statuses = [...new Set(wanted.statuses ?? [])].sort().join(',');
  if (
    (wanted.owner ?? null) !== actual.owner ||
    (wanted.runtime_version ?? null) !== actual.runtime_version ||
    (wanted.agent_id ?? null) !== actual.agent_id ||
    (wanted.sandbox_id != null && wanted.sandbox_id !== actual.sandbox_id) ||
    (wanted.agent_id == null && wanted.sandbox_id == null && actual.sandbox_id !== null) ||
    (wanted.search ?? '').trim().toLowerCase() !== actual.search ||
    statuses !== [...actual.statuses].sort().join(',')
  )
    throw new ProtocolError('usage snapshot filters do not match');
  const seconds =
    window === '24h' ? 86400n : window === '7d' ? 604800n : window === '30d' ? 2592000n : undefined;
  if (seconds !== undefined && BigInt(snapshot.query.to) - BigInt(snapshot.query.from) !== seconds)
    throw new ProtocolError('usage snapshot window does not match');
  return view;
}
