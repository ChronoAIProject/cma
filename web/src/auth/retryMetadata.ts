import { ApiError, TransportError } from '@cma/client';
import type { AuthProvider, AuthRetryMetadata } from './types';

/** Unknown/contradictory HTTP outcomes never become proof that identity is fresh. */
export function authHttpRetry(
  status: number,
  problem: unknown,
  retryAfterMs?: number,
): AuthRetryMetadata | undefined {
  if (
    ![429, 502, 503, 504].includes(status) ||
    typeof problem !== 'object' ||
    problem === null ||
    Array.isArray(problem) ||
    (problem as Record<string, unknown>).retryable !== true
  )
    return undefined;
  return { kind: 'http', status: status as 429 | 502 | 503 | 504, retryAfterMs };
}

export function authTransportRetry(
  error: unknown,
  signal?: AbortSignal,
  timedOut = false,
): AuthRetryMetadata | undefined {
  return !signal?.aborted && (timedOut || error instanceof TypeError)
    ? { kind: 'transport' }
    : undefined;
}

/** Keep provider presentation errors compatible while retaining typed retry evidence. */
export function currentTransportUser(provider: Pick<AuthProvider, 'current'>) {
  return async (signal?: AbortSignal): Promise<string | undefined> => {
    signal?.throwIfAborted();
    const identity = await provider.current(signal);
    signal?.throwIfAborted();
    if (identity.status === 'error') {
      if (identity.retry?.kind === 'transport') throw new TransportError(identity.message);
      if (identity.retry?.kind === 'http')
        throw new ApiError(
          identity.retry.status,
          identity.message,
          undefined,
          undefined,
          identity.retry.retryAfterMs,
        );
      throw new Error(identity.message);
    }
    return identity.status === 'authenticated' ? identity.user.id : undefined;
  };
}
