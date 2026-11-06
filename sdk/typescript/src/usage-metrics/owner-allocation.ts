import type { ResolvedConfig } from '../config.js';
import { ProtocolError, ValidationError } from '../errors.js';
import type { OperationOptions } from '../http.js';
import { query, read, segment } from '../resources.js';
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
import { integer, quantity } from './schema.js';

export interface UsageOwnerAllocationQuery extends OperationOptions {
  readonly metric: 'cpu' | 'memory' | 'disk';
  readonly limit?: number;
  readonly cursor?: string;
}
const owner = object({
  subject: text,
  label: text,
  disambiguator: optional(text),
  allocated_hours: quantity,
  sandbox_count: integer,
  complete: boolean,
});
const page = object({
  report_id: text,
  metric: enumeration('cpu', 'memory', 'disk'),
  unit: enumeration('core-hours', 'GiB-hours'),
  items: array(owner),
  next: nullable(text),
  total_count: integer,
});
export type UsageOwnerAllocations = Output<typeof page>;
export type UsageOwnerAllocation = Output<typeof owner>;

export function ownerAllocations(
  config: ResolvedConfig,
  admin: boolean,
  id: string,
  options: UsageOwnerAllocationQuery,
) {
  if (!admin) throw new ValidationError('owner allocation requires administrator reporting');
  if (
    options.limit !== undefined &&
    (!Number.isInteger(options.limit) || options.limit < 1 || options.limit > 200)
  )
    throw new ValidationError('owner allocation limit must be from 1 through 200');
  return read(
    config,
    query(`/api/v1/admin/usage/metrics/reports/${segment(id)}/owner-allocations`, {
      metric: options.metric,
      limit: options.limit,
      cursor: options.cursor,
    }),
    (value) => {
      const result = page(value);
      if (
        result.report_id !== id ||
        result.metric !== options.metric ||
        result.unit !== (options.metric === 'cpu' ? 'core-hours' : 'GiB-hours')
      )
        throw new ProtocolError('owner allocation identity did not match');
      if (result.items.length > (options.limit ?? 50))
        throw new ProtocolError('owner allocation page exceeds requested limit');
      return result;
    },
    options,
  );
}
