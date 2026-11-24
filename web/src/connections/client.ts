import { ApiError, retryAfterMilliseconds } from '@cma/client';

export const subscriptionService = 'cma:codex-subscription';

export type ConnectionTarget =
  | {
      readonly type: 'sandbox_profile';
      readonly profile: { readonly id: string; readonly revision: number };
    }
  | { readonly type: 'sandbox' | 'workspace' | 'agent' | 'launch'; readonly id: string }
  | { readonly type: 'agent_access_request'; readonly id: string }
  | {
      readonly type: 'agent_service_access';
      readonly id: string;
      readonly services: readonly string[];
    };

export interface ActiveConnection {
  readonly service_id: string;
  readonly intent_id: string;
}

export interface ConnectionRow {
  readonly service: string;
  readonly display_name: string;
  readonly kind: 'nyxid' | 'codex';
  readonly state: 'satisfied' | 'inactive' | 'missing' | 'unknown';
  readonly connection_mode: 'hosted' | 'manual' | 'unknown';
}

export interface ConnectionReport {
  readonly owner: string;
  readonly ready: boolean;
  readonly checked_at: string;
  readonly services: readonly ConnectionRow[];
  readonly active_state?: 'pending' | 'satisfied' | 'expired' | 'cancelled' | 'failed';
  readonly service_authorization_recovery?: boolean;
}

/** An action cannot rely on readiness from a partial or inconsistent response. */
export function isConnectionReport(value: unknown): value is ConnectionReport {
  if (typeof value !== 'object' || value === null) return false;
  const report = value as Partial<ConnectionReport>;
  return (
    typeof report.owner === 'string' &&
    report.owner !== '' &&
    typeof report.ready === 'boolean' &&
    typeof report.checked_at === 'string' &&
    Number.isFinite(Date.parse(report.checked_at)) &&
    Array.isArray(report.services) &&
    report.services.every(
      (row) =>
        row !== null &&
        typeof row === 'object' &&
        typeof row.service === 'string' &&
        typeof row.display_name === 'string' &&
        ['nyxid', 'codex'].includes(row.kind) &&
        ['hosted', 'manual', 'unknown'].includes(row.connection_mode) &&
        (report.ready
          ? row.state === 'satisfied'
          : ['satisfied', 'inactive', 'missing', 'unknown'].includes(row.state)),
    )
  );
}

export interface StartedConnection extends ActiveConnection {
  readonly owner: string;
  readonly hosted_url: string;
  readonly expires_at: string;
  readonly user_code?: string;
}

export interface ConnectionClient {
  check(
    target: ConnectionTarget,
    active?: ActiveConnection,
    signal?: AbortSignal,
  ): Promise<ConnectionReport>;
  start(
    target: ConnectionTarget,
    active: ActiveConnection,
    force?: boolean,
    signal?: AbortSignal,
  ): Promise<StartedConnection>;
  cancel(target: ConnectionTarget, active: ActiveConnection): Promise<void>;
}

export class ConnectionFailure extends ApiError {
  constructor(status: number, code?: string, retryAfterMs?: number) {
    super(
      status,
      status === 401
        ? 'Your CMA sign-in has expired. Sign in again to continue.'
        : code === 'connection_target_not_found'
          ? 'This sandbox or session is no longer available.'
          : code === 'connection_changed'
            ? 'This sign-in attempt has changed. Connect again to continue.'
            : status === 429
              ? 'The service is busy. Please try again shortly.'
              : 'The connection could not be verified. Please try again.',
      code,
      undefined,
      retryAfterMs,
    );
    this.name = 'ConnectionFailure';
  }
}

export function readRepositoryConnections(target: ConnectionTarget, signal: AbortSignal) {
  return post<unknown>('repository-access', { target }, signal);
}

async function post<T>(action: string, body: unknown, signal?: AbortSignal): Promise<T> {
  const response = await fetch(`/auth/connections/${action}`, {
    method: 'POST',
    credentials: 'same-origin',
    cache: 'no-store',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
    signal,
  });
  if (!response.ok) {
    const problem: unknown = await response.json().catch(() => undefined);
    const code =
      typeof problem === 'object' &&
      problem !== null &&
      'code' in problem &&
      typeof problem.code === 'string'
        ? problem.code
        : undefined;
    throw new ConnectionFailure(response.status, code, retryAfterMilliseconds(response));
  }
  return response.json() as Promise<T>;
}

export const connectionClient: ConnectionClient = {
  check: (target, active, signal) =>
    post('check', { target, active: active === undefined ? undefined : intent(active) }, signal),
  start: (target, active, force, signal) =>
    post('start', { target, ...intent(active), force }, signal),
  cancel: async (target, active) => {
    await post('cancel', { target, active: intent(active) });
  },
};

// A started handoff also carries browser-only fields. The server accepts only
// the exact service/intent identity when polling or cancelling that handoff.
function intent(active: ActiveConnection): ActiveConnection {
  return { service_id: active.service_id, intent_id: active.intent_id };
}

export function connectionUrl(raw: string): string | undefined {
  try {
    const url = new URL(raw);
    return url.protocol === 'https:' && !url.username && !url.password ? url.href : undefined;
  } catch {
    return undefined;
  }
}
