import { ApiError } from '@cma/client';
import type { AgentUpdateReason } from '../api/rest/agentUpdateProblem';
import { readRetryDelay } from './transport/readRetry';

export interface SettingsFailure {
  readonly kind: 'invalid' | 'denied' | 'conflict' | 'unknown_conflict' | 'retry';
  readonly message: string;
  readonly retryAfterMs: number;
}

const recordedConflict = {
  dispatch_in_progress:
    'A dispatch was in progress when this update was refused. Wait for it to settle, then refresh before applying again.',
  dispatch_recovery_required:
    'A prior dispatch needed recovery when this update was refused. Refresh after it has been reconciled.',
  stale_revision: 'Preferences changed before this update. Refresh preferences, then apply again.',
  admission_conflict: 'The Agent changed during the update. Refresh before applying again.',
  agent_not_idle:
    'The Agent was not ready for this change. Wait until it is ready, then refresh before applying again.',
} satisfies Partial<Record<AgentUpdateReason, string>>;

function recordedConflictMessage(error: ApiError): string | undefined {
  const reason = error.extensions.settings_update_reason;
  if (error.code === 'agent_not_ready' && reason === 'agent_not_idle')
    return recordedConflict.agent_not_idle;
  if (error.code !== 'agent_operation_in_flight') return undefined;
  switch (reason) {
    case 'dispatch_in_progress':
    case 'dispatch_recovery_required':
    case 'stale_revision':
    case 'admission_conflict':
      return recordedConflict[reason];
    default:
      return undefined;
  }
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
  if (!reading && error instanceof ApiError && status === 409) {
    const message = recordedConflictMessage(error);
    return message
      ? { kind: 'conflict', retryAfterMs, message }
      : {
          kind: 'unknown_conflict',
          retryAfterMs,
          message: 'The update outcome is unconfirmed. Refresh, then retry the same request.',
        };
  }
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
  if (
    !reading &&
    status === 503 &&
    error instanceof ApiError &&
    error.code === 'agent_owner_unavailable'
  )
    return {
      kind: 'retry',
      retryAfterMs,
      message:
        'Agent Manager could not confirm the update with the server responsible for this Agent. Your changes are kept. Retry the same request when the connection recovers.',
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
  // An endpoint's typed pre-write refusal is recorded under the exact key.
  // Generic validation or access errors can be returned before receipt lookup.
  const rejected = failure.kind === 'conflict' || (!unresolved && failure.kind === 'invalid');
  return {
    ...failure,
    outcome: rejected ? ('rejected' as const) : ('unconfirmed' as const),
    refreshRequired:
      failure.kind === 'denied' ||
      failure.kind === 'conflict' ||
      failure.kind === 'unknown_conflict' ||
      (unresolved && failure.kind === 'invalid'),
    message:
      unresolved && failure.kind === 'invalid'
        ? 'This attempt was refused, but the earlier update is still unconfirmed. Refresh current settings, then retry the same request.'
        : failure.message,
  };
}
