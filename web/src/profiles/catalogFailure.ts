import {
  ApiError,
  CredentialError,
  ProtocolError,
  TransportError,
  ValidationError,
} from '@cma/client';

export interface CatalogFailure {
  readonly kind:
    | 'sign_in'
    | 'forbidden'
    | 'service_authorization'
    | 'rate_limited'
    | 'unavailable'
    | 'invalid_response'
    | 'invalid_query';
  readonly message: string;
  readonly retry: boolean;
  readonly retryAfterMs?: number;
  readonly status?: number;
  readonly code?: string;
  readonly requestId?: string;
}

const codes = new Set([
  'not_authenticated',
  'forbidden',
  'skill_catalog_authorization_required',
  'rate_limited',
  'profile_skills_unavailable',
  'profile_services_unavailable',
  'invalid_profile_authoring_query',
  'auth_temporarily_unavailable',
]);

/** The service's free-form message and extensions are never presentation data. */
export function catalogFailure(cause: unknown): CatalogFailure {
  const diagnostics =
    cause instanceof ApiError
      ? {
          status:
            Number.isInteger(cause.status) && cause.status >= 400 && cause.status <= 599
              ? cause.status
              : undefined,
          code: cause.code !== undefined && codes.has(cause.code) ? cause.code : undefined,
          retryAfterMs:
            cause.retryAfterMs !== undefined &&
            Number.isFinite(cause.retryAfterMs) &&
            cause.retryAfterMs > 0
              ? cause.retryAfterMs
              : undefined,
          requestId:
            typeof cause.requestId === 'string' &&
            /^[A-Za-z0-9][A-Za-z0-9_.:-]{0,63}$/.test(cause.requestId)
              ? cause.requestId
              : undefined,
        }
      : {};
  if (cause instanceof CredentialError || (cause instanceof ApiError && cause.status === 401))
    return {
      ...diagnostics,
      kind: 'sign_in',
      message: 'Sign in again, then reopen this catalog.',
      retry: false,
    };
  if (cause instanceof ApiError) {
    if (cause.status === 403 && cause.code === 'skill_catalog_authorization_required')
      return {
        ...diagnostics,
        kind: 'service_authorization',
        message: 'Authorize Ornn access and sign in again to load the skills catalog.',
        retry: false,
      };
    if (cause.status === 403)
      return {
        ...diagnostics,
        kind: 'forbidden',
        message:
          'You do not have permission to read this catalog. Reading your catalog requires permission to use your own Sandboxes or manage your own profiles.',
        retry: false,
      };
    if (cause.status === 429)
      return {
        ...diagnostics,
        kind: 'rate_limited',
        message: 'Too many catalog requests. Wait before trying again.',
        retry: true,
      };
    if (cause.status === 400 && cause.code === 'invalid_profile_authoring_query')
      return {
        ...diagnostics,
        kind: 'invalid_query',
        message: 'This catalog query is not valid. Change the search text and try again.',
        retry: false,
      };
  }
  if (cause instanceof ValidationError)
    return {
      kind: 'invalid_query',
      message: 'This catalog query is not valid. Change the search text and try again.',
      retry: false,
    };
  if (cause instanceof ProtocolError)
    return {
      kind: 'invalid_response',
      message:
        'The catalog returned an invalid response. Try again. If this continues, report the catalog problem.',
      retry: true,
    };
  if (cause instanceof TransportError)
    return {
      kind: 'unavailable',
      message: 'The catalog could not be reached. Check your connection and try again.',
      retry: true,
    };
  return {
    ...diagnostics,
    kind: 'unavailable',
    message: 'The catalog is temporarily unavailable. Try again.',
    retry: true,
  };
}
