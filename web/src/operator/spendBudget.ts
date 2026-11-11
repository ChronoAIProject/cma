// The admin settings API owns these integer accounting units; no currency is inferred.
export type Amount = number | string;
export interface PlatformBudget {
  period_cap: Amount;
  period_start: string;
  period_end: string;
  warning_percent: Amount;
  stop_percent: Amount;
  protected_reserve: Amount;
  lag_allowance: Amount;
}
export interface OwnerBudget {
  owner_day_limit: Amount;
  turn_limit: Amount;
  max_calls_per_turn: Amount;
  day_timezone: string;
  input_token_units: Amount;
  output_token_units: Amount;
  max_unobserved_units: Amount;
  usage_timeout_seconds: Amount;
}
export interface SpendBudget {
  enabled: boolean;
  platform?: PlatformBudget;
  default_policy?: OwnerBudget;
  owners: { owner: string; policy: OwnerBudget }[];
}
export const platformAmounts = {
  period_cap: 'Platform period cap (accounting units)',
  warning_percent: 'Warning threshold (%)',
  stop_percent: 'Hard-stop threshold (%)',
  protected_reserve: 'Protected reserve for existing users (accounting units)',
  lag_allowance: 'Unmetered-lag allowance (accounting units)',
} as const;
export const ownerAmounts = {
  owner_day_limit: 'Per-owner daily cap (accounting units)',
  turn_limit: 'Per-turn cap (accounting units)',
  max_calls_per_turn: 'Maximum model calls per turn',
  input_token_units: 'Input token valuation (accounting units per token)',
  output_token_units: 'Output token valuation (accounting units per token)',
  max_unobserved_units: 'Unobserved turn allowance (accounting units)',
  usage_timeout_seconds: 'Usage reporting timeout (seconds)',
} as const;
export const blankPlatform = (): PlatformBudget => ({
  period_cap: '',
  period_start: '',
  period_end: '',
  warning_percent: '',
  stop_percent: '',
  protected_reserve: '',
  lag_allowance: '',
});
export const blankOwner = (): OwnerBudget => ({
  owner_day_limit: '',
  turn_limit: '',
  max_calls_per_turn: '',
  day_timezone: '',
  input_token_units: '',
  output_token_units: '',
  max_unobserved_units: '',
  usage_timeout_seconds: '',
});
function object(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
function policyShape(value: unknown, amounts: object, texts: string[]) {
  return (
    object(value) &&
    Object.keys(amounts).every(
      (key) => typeof value[key] === 'number' || typeof value[key] === 'string',
    ) &&
    texts.every((key) => typeof value[key] === 'string')
  );
}
export function spendBudget(value: unknown): SpendBudget | undefined {
  if (!object(value) || typeof value.enabled !== 'boolean' || !Array.isArray(value.owners)) return;
  if (
    value.platform !== undefined &&
    !policyShape(value.platform, platformAmounts, ['period_start', 'period_end'])
  )
    return;
  if (
    value.default_policy !== undefined &&
    !policyShape(value.default_policy, ownerAmounts, ['day_timezone'])
  )
    return;
  if (
    !value.owners.every(
      (entry) =>
        object(entry) &&
        typeof entry.owner === 'string' &&
        policyShape(entry.policy, ownerAmounts, ['day_timezone']),
    )
  )
    return;
  return value as unknown as SpendBudget;
}
// Retain invalid text for correction; never round a decimal or unsafe integer into a cap.
export function exactAmount(text: string): Amount {
  if (!/^(0|[1-9]\d*)$/.test(text)) return text;
  const value = Number(text);
  return Number.isSafeInteger(value) ? value : text;
}
export const budgetChanged = (before: unknown, after: unknown) =>
  JSON.stringify(before) !== JSON.stringify(after);

export function reviewSpendBudget(before: unknown, edited: unknown, latest: unknown): unknown {
  if (!budgetChanged(before, edited)) return latest;
  const base = spendBudget(before),
    draft = spendBudget(edited),
    current = spendBudget(latest);
  if (!base || !draft || !current) return latest;
  function policy<T extends PlatformBudget | OwnerBudget>(
    old: T | undefined,
    next: T | undefined,
    now: T | undefined,
  ): T | undefined {
    if (!budgetChanged(old, next)) return now;
    if (!old || !next) return next;
    const result = { ...now } as T;
    for (const key of Object.keys(next) as (keyof T)[]) {
      if (budgetChanged(old[key], next[key])) result[key] = next[key];
    }
    // Removed policies require all values to be reviewed again, not restored from stale base.
    return {
      ...(Object.hasOwn(next, 'period_cap') ? blankPlatform() : blankOwner()),
      ...result,
    } as T;
  }
  return {
    ...current,
    enabled: base.enabled === draft.enabled ? current.enabled : draft.enabled,
    platform: policy(base.platform, draft.platform, current.platform),
    default_policy: policy(base.default_policy, draft.default_policy, current.default_policy),
  };
}
