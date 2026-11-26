import type { ResolvedConfig } from '../config.js';
import { ApiError } from '../errors.js';
import { query, read } from '../resources.js';
import { activitySummary, adminPage } from './types.js';
import type { ActivityListOptions } from './index.js';

export function listActivitySummary(config: ResolvedConfig, options: ActivityListOptions) {
  const request = (excludeSuccessfulReads: boolean, excludeCompatibilityProbes: boolean) =>
    read(
      config,
      query('/api/v1/admin/activity', {
        view: 'summary',
        actor: options.actor,
        action: options.action,
        exclude_successful_reads: excludeSuccessfulReads ? 'true' : undefined,
        exclude_compatibility_probes: excludeCompatibilityProbes ? 'true' : undefined,
        cursor: options.cursor,
        limit: options.limit,
      }),
      adminPage(activitySummary),
      options,
    );
  const successful = options.excludeSuccessfulReads === true;
  const compatibility = options.excludeCompatibilityProbes === true;
  const unsupported = (error: unknown) =>
    error instanceof ApiError &&
    error.status === 400 &&
    error.code === 'invalid_admin_request' &&
    !options.signal?.aborted;
  const olderRequest = () =>
    request(successful, false).catch((error: unknown) => {
      if (successful && unsupported(error)) return request(false, false);
      throw error;
    });
  if (compatibility)
    return request(successful, true).catch((error: unknown) => {
      if (unsupported(error)) return olderRequest();
      throw error;
    });
  return olderRequest();
}
