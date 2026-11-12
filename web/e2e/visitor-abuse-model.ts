import { object } from './environment';

export function check(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(`TC-117 ${message}`);
}
export const blocked = {
  instances: 'BLOCKED: TC-117 requires CMA_E2E_INSTANCE_ORIGINS with two distinct serving origins',
  utc: 'BLOCKED: TC-117 UTC campaign must start in the 30 seconds before real UTC midnight',
  utcWindow: 'BLOCKED: TC-117 pre/post-midnight operations exceeded the real UTC 30-second window',
  budget: 'BLOCKED: TC-117 requires a frozen load budget and at least two real egress proxies',
  separation:
    'BLOCKED: TC-117 independent egress networks and offered load must exceed the fleet burst and rate without being masked by old network buckets',
} as const;
export function requireInstances(origins: string[]): [string, string] {
  if (origins.length !== 2 || new Set(origins).size !== 2) throw new Error(blocked.instances);
  return origins as [string, string];
}
export function utcBoundary(now = Date.now()): number {
  const midnight = (Math.floor(now / 86_400_000) + 1) * 86_400_000;
  if (midnight - now > 30_000) throw new Error(blocked.utc);
  return midnight;
}
export interface Result {
  status: number;
  body: Record<string, unknown>;
  retryAfter?: string;
  ms: number;
}
export function assertQuotaRefusal(result: Pick<Result, 'status' | 'body' | 'retryAfter'>) {
  const reset = Date.parse(String(result.body.reset_at));
  check(
    result.status === 429 &&
      result.body.code === 'resource_quota' &&
      result.body.resource === 'visitor_launches' &&
      result.body.limit === 5 &&
      Number.isFinite(reset) &&
      reset % 86_400_000 === 0 &&
      Number.isFinite(Number(result.retryAfter)) &&
      Number(result.retryAfter) > 0,
    'expected typed daily quota refusal with limit, UTC reset_at and Retry-After',
  );
}
export interface Receipt {
  id: string;
  created_at: string;
  public_id?: string;
  state?: string;
  failure?: { code: string };
  resolution?: Record<string, { state: string; id?: string }>;
}
export function receipt(body: Record<string, unknown>): Receipt {
  check(
    typeof body.id === 'string' && Number.isFinite(Date.parse(String(body.created_at))),
    'invalid public Launch receipt',
  );
  return body as unknown as Receipt;
}
export function admissionDays(rows: Pick<Receipt, 'id' | 'created_at'>[]) {
  const days: Record<string, number> = {};
  for (const row of new Map(rows.map((row) => [row.id, row])).values()) {
    const time = Date.parse(row.created_at);
    check(Number.isFinite(time), 'invalid admission created_at');
    const day = new Date(time).toISOString().slice(0, 10);
    days[day] = (days[day] ?? 0) + 1;
    check(days[day] <= 5, 'daily durable admission overshoot');
  }
  return days;
}
export interface ControlSample {
  kind: 'stop' | 'cancel';
  status: number;
  ms: number;
}
export function p99(values: number[]) {
  return values.length
    ? [...values].sort((a, b) => a - b)[Math.ceil(values.length * 0.99) - 1]!
    : null;
}
export function assertControls(samples: ControlSample[]) {
  for (const kind of ['stop', 'cancel']) {
    const rows = samples.filter((row) => row.kind === kind);
    check(
      rows.length > 0 && rows.every((row) => row.status >= 200 && row.status < 300),
      `${kind} success rate must be 100 percent`,
    );
    check(p99(rows.map((row) => row.ms))! <= 1000, `${kind} p99 must be at most 1000 ms`);
  }
}
export function newCounts() {
  return {
    overquotaAttempts: 0,
    overquotaRejects: 0,
    idempotencyChecks: 0,
    idempotentChargeDuplicates: 0,
    sideEffectChecks: 0,
    anonymousSideEffects: 0,
    collateralAttempts: 0,
    collateralErrors: 0,
    offered: 0,
    admitted: 0,
    refused: 0,
    transportErrors: 0,
  };
}
export type Counts = ReturnType<typeof newCounts>;
export function metrics(counts: Counts) {
  return {
    'visitor_overquota_rejects/attempts': counts.overquotaAttempts
      ? counts.overquotaRejects / counts.overquotaAttempts
      : null,
    idempotent_charge_duplicates: counts.idempotencyChecks
      ? counts.idempotentChargeDuplicates
      : null,
    anonymous_side_effects: counts.sideEffectChecks ? counts.anonymousSideEffects : null,
    collateral_error_rate: counts.collateralAttempts
      ? counts.collateralErrors / counts.collateralAttempts
      : null,
  };
}
export function count(value: unknown): number {
  check(Number.isSafeInteger(value) && Number(value) >= 0, 'invalid public counter');
  return Number(value);
}
export function stats(body: Record<string, unknown>) {
  const totals = object(body.totals, 'Trigger statistics totals');
  return { launches: count(totals.launches_total), ready: count(totals.agents_ready_total) };
}

export function assertAnonymousBudget(
  admitted: number,
  elapsedMs: number,
  perPodPerMinute: number,
  maxServingPods: number,
) {
  const ceiling = Math.floor(perPodPerMinute * maxServingPods * (1 + elapsedMs / 60000));
  check(
    admitted <= ceiling,
    'anonymous admissions exceeded the frozen fleet rate and initial burst',
  );
}

export function assertLoadSeparation(
  budget: {
    perPodPerMinute: number;
    maxServingPods: number;
    networkPerMinute: number;
    aboveRps: number;
    belowRps: number;
    durationSeconds: number;
  },
  networks: number,
) {
  const seconds = budget.durationSeconds;
  const offered = budget.aboveRps * seconds;
  const fleet = budget.perPodPerMinute * budget.maxServingPods * (1 + seconds / 60);
  const oldNetworkCapacity =
    networks *
    Math.min(
      Math.floor(offered / networks),
      Math.floor(budget.networkPerMinute * (1 + seconds / 60)) -
        Math.ceil((budget.belowRps * seconds) / networks),
    );
  if (offered <= fleet || oldNetworkCapacity <= fleet) throw new Error(blocked.separation);
}
