import { ApiError } from '@chronoai/cma-sdk';
import { UsageBackoff } from './request';

export interface MetricsFailure {
  readonly title: string;
  readonly message: string;
  readonly retryAt?: number;
  readonly denied: boolean;
  readonly expired: boolean;
}
export function metricsFailure(error: unknown): MetricsFailure {
  const base = { denied: false, expired: false };
  if (error instanceof UsageBackoff)
    return {
      ...base,
      title: 'Usage reporting is busy',
      message: 'Wait before retrying. Your selection is preserved.',
      retryAt: error.retryAt,
    };
  if (error instanceof ApiError) {
    if (error.status === 401 || error.status === 403 || error.status === 404)
      return {
        ...base,
        denied: true,
        title: 'Usage access unavailable',
        message:
          'Your current account cannot read this usage selection. Check your access or choose another selection.',
      };
    if (error.status === 410)
      return {
        ...base,
        expired: true,
        title: 'Usage report expired',
        message: 'Refresh to create a new report for this selection.',
      };
    if (error.status === 429)
      return {
        ...base,
        title: 'Usage reporting is busy',
        message: 'Wait before retrying. Your selection is preserved.',
        retryAt: Date.now() + Math.max(1000, error.retryAfterMs ?? 60000),
      };
    if (error.status === 503)
      return {
        ...base,
        title: 'Usage reporting is unavailable',
        message: 'Usage could not be loaded. Try again later; unavailable data is not zero usage.',
      };
  }
  return { ...base, title: 'Could not load usage', message: 'Retry to load this selection.' };
}
