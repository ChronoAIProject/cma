import { ProtocolError } from '../errors.js';
import {
  array,
  boolean,
  enumeration,
  nullable,
  object,
  optional,
  text,
  type Output,
} from '../schema.js';

/** Decimal strings preserve integer quantities beyond JavaScript precision. */
export type UsageMetricQuantity = number | string;
export const quantity = (value: unknown): UsageMetricQuantity => {
  if (
    typeof value === 'string' &&
    /^[1-9][0-9]{15,308}$/.test(value) &&
    BigInt(value) > BigInt(Number.MAX_SAFE_INTEGER)
  )
    return value;
  if (
    typeof value !== 'number' ||
    !Number.isFinite(value) ||
    value < 0 ||
    (Number.isInteger(value) && !Number.isSafeInteger(value))
  )
    throw new ProtocolError(
      'expected a finite nonnegative quantity or a large decimal integer string',
    );
  return value;
};
const integer = (value: unknown): UsageMetricQuantity => {
  const parsed = quantity(value);
  if (
    (typeof parsed === 'number' && !Number.isSafeInteger(parsed)) ||
    (typeof parsed === 'string' && BigInt(parsed) > 18446744073709551615n)
  )
    throw new ProtocolError('expected an unsigned 64-bit integer');
  return parsed;
};
export const range = object({ from: integer, to: integer });
const status = enumeration(
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
);
const filters = object({
  owner: nullable(text),
  statuses: array(status),
  runtime_version: nullable(text),
  sandbox_id: nullable(text),
  agent_id: nullable(text),
  search: text,
});
const query = object({ from: integer, to: integer, step_seconds: integer, filters });
const capacity = object({ cpu_cores: quantity, memory_bytes: quantity, disk_bytes: quantity });
const allocated = object({
  disk_retained_seconds: quantity,
  uptime_seconds: quantity,
  cpu_core_hours: quantity,
  memory_gib_hours: quantity,
  disk_gib_hours: quantity,
  complete: boolean,
  projected_tail_seconds: integer,
  unknown_seconds: integer,
});
const measurement = {
  state: enumeration(
    'ok',
    'partial',
    'stale',
    'unmounted',
    'not_running',
    'unavailable',
    'unattributed',
  ),
  observed_at: nullable(quantity),
  source: text,
  covered_seconds: quantity,
  expected_seconds: quantity,
  unknown_mapping_seconds: quantity,
  coverage_ratio: nullable(quantity),
  available_from: nullable(quantity),
  last_observed: nullable(quantity),
  percent_of_limit: nullable(quantity),
  filesystem_capacity_bytes: nullable(quantity),
  exceeds_capacity: boolean,
};
const cpu = object({
  ...measurement,
  latest_cores: nullable(quantity),
  mean_cores: nullable(quantity),
  max_cores: nullable(quantity),
  observed_core_hours: nullable(quantity),
});
const gauge = object({
  ...measurement,
  latest_bytes: nullable(quantity),
  mean_bytes: nullable(quantity),
  max_bytes: nullable(quantity),
  observed_gib_hours: nullable(quantity),
});
const measured = object({ cpu, memory: gauge, disk: gauge });
const billing = object({
  basis: enumeration('allocated'),
  status: enumeration('not_integrated'),
  charges: nullable(quantity),
});
export const row = object({
  sandbox_id: text,
  name: text,
  owner: object({ subject: text, display_name: optional(text) }),
  status,
  runtime_version: nullable(text),
  agent_count: integer,
  last_activity_at: nullable(integer),
  allocation: capacity,
  allocated_period: allocated,
  measured,
  billing,
  shared_sandbox_usage: boolean,
});
const infrastructure = object({
  state: text,
  unproven_count: integer,
  unassigned_pool: array(nullable(quantity)),
  unattributed: array(nullable(quantity)),
  observed_at: nullable(integer),
});
const summary = object({
  sandbox_count: integer,
  running_count: integer,
  allocated_period: allocated,
  total_allocated_capacity: capacity,
  covered_allocated_capacity: capacity,
  measured: nullable(measured),
  excluded_count: integer,
  infrastructure: nullable(infrastructure),
});
export const report = object({
  schema_version: integer,
  report_id: text,
  status: enumeration('pending', 'ready', 'failed'),
  report_complete: boolean,
  scope: object({ principal: text, admin: boolean }),
  query,
  as_of: integer,
  measurement_cutoff: integer,
  effective_range: range,
  expires_at: integer,
  allocation_watermark: text,
  mapping_revision: integer,
  available_from: nullable(integer),
  tracking_started_at: nullable(integer),
  warnings: array(text),
  source_health: text,
  summary: nullable(summary),
  poll_url: text,
  retry_after_seconds: integer,
});
export const page = object({
  items: array(row),
  next: nullable(text),
  report_id: text,
  as_of: integer,
  total_count: integer,
});
const point = object({
  from: integer,
  to: integer,
  mean: nullable(quantity),
  min: nullable(quantity),
  max: nullable(quantity),
  covered_seconds: quantity,
  expected_seconds: quantity,
  unknown_mapping_seconds: quantity,
  numerator: nullable(quantity),
  denominator: nullable(quantity),
  reason: nullable(text),
  excluded_count: integer,
});
export const series = object({
  report_id: text,
  metric: enumeration('cpu', 'memory', 'disk'),
  unit: enumeration('cores', 'bytes'),
  source: text,
  lines: array(object({ id: text, label: text, points: array(point) })),
});
export type UsageMetricReport = Output<typeof report>;
export type UsageMetricRow = Output<typeof row>;
export type UsageMetricPage = Output<typeof page>;
export type UsageMetricSeries = Output<typeof series>;
export type UsageMetricStatus = Output<typeof status>;

export const owners = object({
  items: array(object({ subject: text, display_name: optional(text) })),
  next: nullable(text),
  report_id: text,
});
export type UsageMetricOwners = Output<typeof owners>;
