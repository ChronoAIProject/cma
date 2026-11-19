import { ApiError, CredentialError, ProtocolError, TransportError } from '../api-client/errors';

export interface CatalogFailure {
  readonly kind:
    'sign-in' | 'forbidden' | 'rate-limit' | 'unavailable' | 'invalid' | 'network' | 'unknown';
  readonly message: string;
  readonly retryable: boolean;
  readonly retryAfterMs?: number;
  readonly status?: number;
  readonly code?: string;
  readonly requestId?: string;
}

const codes = new Set([
  'not_authenticated',
  'forbidden',
  'profile_services_unavailable',
  'profile_skills_unavailable',
  'rate_limited',
  'invalid_profile_authoring_query',
]);

export function catalogFailure(error: unknown): CatalogFailure {
  const diagnostics =
    error instanceof ApiError
      ? {
          status:
            Number.isInteger(error.status) && error.status >= 100 && error.status <= 599
              ? error.status
              : undefined,
          code: error.code !== undefined && codes.has(error.code) ? error.code : undefined,
          requestId:
            error.requestId !== undefined &&
            /^[a-zA-Z0-9][a-zA-Z0-9._:-]{0,63}$/.test(error.requestId)
              ? error.requestId
              : undefined,
          retryAfterMs:
            error.retryAfterMs !== undefined &&
            Number.isFinite(error.retryAfterMs) &&
            error.retryAfterMs > 0
              ? error.retryAfterMs
              : undefined,
        }
      : {};
  let failure: CatalogFailure;
  if (error instanceof CredentialError || (error instanceof ApiError && error.status === 401)) {
    failure = { kind: 'sign-in', message: 'Sign in again to read your catalog.', retryable: false };
  } else if (error instanceof ApiError && error.status === 403) {
    failure = {
      kind: 'forbidden',
      message:
        'Reading your catalog requires permission to use your own Sandboxes or manage your own profiles.',
      retryable: false,
    };
  } else if (error instanceof ApiError && error.status === 429) {
    failure = {
      kind: 'rate-limit',
      message: 'Too many catalog requests. Wait before trying again.',
      retryable: true,
    };
  } else if (error instanceof ApiError && error.status >= 500) {
    failure = {
      kind: 'unavailable',
      message: 'The catalog is temporarily unavailable. Try again.',
      retryable: true,
    };
  } else if (error instanceof ProtocolError) {
    failure = {
      kind: 'invalid',
      message: 'The catalog response could not be verified. Try again.',
      retryable: true,
    };
  } else if (error instanceof TransportError) {
    failure = {
      kind: 'network',
      message: 'The catalog could not be reached. Check your connection and try again.',
      retryable: true,
    };
  } else {
    failure = {
      kind: 'unknown',
      message: 'The catalog could not be loaded. Try again.',
      retryable: true,
    };
  }
  return { ...failure, ...diagnostics };
}
