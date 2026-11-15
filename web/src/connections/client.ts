import { ApiError, retryAfterMilliseconds } from '@chronoai/cma-sdk';

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
  /** Presentation evidence only. Every Send still uses the fresh check endpoint. */
  readonly verification?: 'cached' | 'fresh';
  readonly valid_for_ms?: number;
}

export interface StartedConnection extends ActiveConnection {
  readonly owner: string;
  readonly hosted_url: string;
  readonly expires_at: string;
  readonly user_code?: string;
}

export interface ConnectionClient {
  open(
    target: { readonly type: 'agent'; readonly id: string },
    signal?: AbortSignal,
  ): Promise<ConnectionReport>;
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
  open: async (target, signal) => {
    const started = performance.now();
    try {
      const report = await post<ConnectionReport>('open', { target }, signal);
      signal?.throwIfAborted();
      if (report.verification !== 'fresh' && report.verification !== 'cached')
        throw new ConnectionFailure(503, 'connection_unavailable');
      const elapsed = performance.now() - started;
      if (
        report.verification === 'cached' &&
        (!Number.isFinite(report.valid_for_ms) ||
          report.valid_for_ms! > 30_000 ||
          elapsed < 0 ||
          report.valid_for_ms! <= elapsed)
      )
        return connectionClient.check(target, undefined, signal);
      return report.verification === 'cached'
        ? { ...report, valid_for_ms: report.valid_for_ms! - elapsed }
        : report;
    } catch (error) {
      // Old AMs do not implement Open readiness; refusals and throttles never
      // fall back to a different authority or bypass their retry deadline.
      if (error instanceof ConnectionFailure && error.status === 404 && error.code === undefined)
        return connectionClient.check(target, undefined, signal);
      throw error;
    }
  },
  check: async (target, active, signal) => {
    const report = await post<ConnectionReport>(
      'check',
      { target, active: active === undefined ? undefined : intent(active) },
      signal,
    );
    if (report.verification === 'cached')
      throw new ConnectionFailure(503, 'connection_unavailable');
    return report;
  },
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
