import type { ResolvedConfig } from '../config.js';
import { ValidationError } from '../errors.js';
import type { OperationOptions } from '../http.js';
import { read } from '../resources.js';
import { array, nullable, object, text, type Output } from '../schema.js';

export interface UsageFilterOptionsQuery extends OperationOptions {
  readonly kind: 'owner' | 'sandbox' | 'runtime';
  readonly search?: string;
  readonly owner?: string;
  readonly selected?: string;
  readonly cursor?: string;
  readonly limit?: number;
}
const optionsPage = object({
  items: array(object({ value: text, label: text })),
  next: nullable(text),
});
export type UsageFilterOptions = Output<typeof optionsPage>;

export function filterOptions(
  config: ResolvedConfig,
  admin: boolean,
  options: UsageFilterOptionsQuery,
) {
  if (!admin) throw new ValidationError('filter options require administrator reporting');
  if (
    options.limit !== undefined &&
    (!Number.isInteger(options.limit) || options.limit < 1 || options.limit > 50)
  )
    throw new ValidationError('option limit must be from 1 through 50');
  // Names belong in the body, never in proxy/access-log URLs.
  return read(config, '/api/v1/admin/usage/metrics/options', optionsPage, {
    ...options,
    method: 'POST',
    body: {
      kind: options.kind,
      search: options.search,
      owner: options.owner,
      selected: options.selected,
      cursor: options.cursor,
      limit: options.limit,
    },
  });
}
