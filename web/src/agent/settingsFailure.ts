import { ApiError } from '@cma/client';
import { readRetryDelay } from './transport/readRetry';

export interface SettingsFailure {
  readonly kind: 'invalid' | 'denied' | 'conflict' | 'retry';
  readonly message: string;
  readonly retryAfterMs: number;
}

export function settingsFailure(error: unknown, reading = false): SettingsFailure {
  const status = error instanceof ApiError ? error.status : undefined;
  const serverDelay = error instanceof ApiError ? error.retryAfterMs : undefined;
  const retryAfterMs =
    serverDelay !== undefined &&
    Number.isSafeInteger(serverDelay) &&
    serverDelay > 0 &&
    serverDelay <= 2_147_483_647
      ? serverDelay
      : 0;
  if (status === 401 || status === 403 || status === 404 || status === 410)
    return {
      kind: 'denied',
      retryAfterMs,
      message:
        status === 401
          ? 'Sign in again, then refresh to check access before saving. Your changes are kept.'
          : 'Access to these settings was refused. Refresh to check access before saving. Your changes are kept.',
    };
  if (
    error instanceof ApiError &&
    ['agent_delivery_ambiguous', 'idempotency_in_flight', 'idempotency_recovery_required'].includes(
      error.code ?? '',
    )
  )
    return {
      kind: 'retry',
      retryAfterMs,
      message: 'The update outcome is unconfirmed. Retry the same request to check it.',
    };
  if (status === 409)
    return {
      kind: 'conflict',
      retryAfterMs,
      message:
        'The Agent state changed or an update is in progress. Refresh the current settings, then apply your changes again.',
    };
  if (status === 400 || status === 422)
    return {
      kind: 'invalid',
      retryAfterMs,
      message: 'The update was rejected. Check your changes, then try again.',
    };
  if (status === 429)
    return {
      kind: 'retry',
      retryAfterMs: retryAfterMs || readRetryDelay(error, 1),
      message:
        'Too many requests. Your changes are kept. Retry the same request when the wait ends.',
    };
  return {
    kind: 'retry',
    retryAfterMs,
    message: reading
      ? 'Current settings could not be read. Your changes are kept. Refresh to try again.'
      : 'The update outcome is unconfirmed. Retry the same request to check it.',
  };
}

export function settingsWriteFailure(error: unknown, unresolved: boolean) {
  const failure = settingsFailure(error);
  const rejected = failure.kind === 'invalid' || failure.kind === 'conflict';
  // A refusal can precede command lookup; access can also fail after execution.
  // Neither settles an earlier uncertain outcome.
  return {
    ...failure,
    outcome: !unresolved && rejected ? ('rejected' as const) : ('unconfirmed' as const),
    refreshRequired:
      failure.kind === 'denied' || failure.kind === 'conflict' || (unresolved && rejected),
    message:
      unresolved && rejected
        ? 'This attempt was refused, but the earlier update is still unconfirmed. Refresh current settings, then retry the same request.'
        : failure.message,
  };
}
