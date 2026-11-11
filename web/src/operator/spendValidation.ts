import { ownerAmounts, platformAmounts, type SpendBudget } from './spendBudget';

export type SpendErrors = Record<string, string>;
const safe = (value: unknown): value is number =>
  typeof value === 'number' && Number.isSafeInteger(value);

function instant(value: string): number {
  const match =
    /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d+)?(Z|[+-]\d{2}:\d{2})$/i.exec(value);
  if (!match || !match[7]) return NaN;
  const [, year, month, day, hour, minute, second] = match;
  const zone = match[7];
  const days = new Date(Date.UTC(Number(year), Number(month), 0)).getUTCDate();
  if (
    Number(month) < 1 ||
    Number(month) > 12 ||
    Number(day) < 1 ||
    Number(day) > days ||
    Number(hour) > 23 ||
    Number(minute) > 59 ||
    Number(second) > 59
  )
    return NaN;
  if (zone !== 'Z' && zone !== 'z' && (Number(zone.slice(1, 3)) > 23 || Number(zone.slice(4)) > 59))
    return NaN;
  return Date.parse(value);
}
function timezone(value: string) {
  if (!/^[A-Za-z][A-Za-z0-9_+/-]*$/.test(value)) return false;
  try {
    new Intl.DateTimeFormat('en', { timeZone: value });
    return true;
  } catch {
    return false;
  }
}

export function validateSpendBudget(budget: SpendBudget): SpendErrors {
  const errors: SpendErrors = {};
  function integer(path: string, value: unknown, min = 1, max = Number.MAX_SAFE_INTEGER) {
    if (!safe(value) || value < min || value > max)
      errors[path] = `Enter a whole number from ${min} to ${max}, without decimals or exponents.`;
  }
  if (
    budget.enabled &&
    (!budget.platform || (!budget.default_policy && budget.owners.length === 0))
  )
    errors.enabled =
      'Enabling requires a complete platform policy and a default or explicit owner policy.';
  const platform = budget.platform;
  if (platform) {
    for (const key of Object.keys(platformAmounts) as (keyof typeof platformAmounts)[])
      integer(
        `platform.${key}`,
        platform[key],
        key === 'protected_reserve' || key === 'lag_allowance' ? 0 : 1,
        key.endsWith('_percent') ? 100 : Number.MAX_SAFE_INTEGER,
      );
    if (
      safe(platform.warning_percent) &&
      safe(platform.stop_percent) &&
      platform.warning_percent >= platform.stop_percent
    )
      errors['platform.warning_percent'] = 'Warning must be below the hard-stop percentage.';
    const {
      period_cap: cap,
      stop_percent: stop,
      protected_reserve: reserve,
      lag_allowance: lag,
    } = platform;
    if (
      [cap, stop, reserve, lag].every(safe) &&
      !Object.keys(errors).some((key) => key.startsWith('platform.'))
    ) {
      const threshold = (BigInt(cap) * BigInt(stop)) / 100n;
      if (BigInt(reserve) + BigInt(lag) >= threshold)
        errors['platform.protected_reserve'] =
          'Reserve plus lag allowance must be below the hard-stop threshold.';
    }
    for (const key of ['period_start', 'period_end'] as const)
      if (!Number.isFinite(instant(platform[key])))
        errors[`platform.${key}`] =
          'Enter a valid RFC3339 instant with an explicit timezone offset.';
    if (
      !errors['platform.period_start'] &&
      !errors['platform.period_end'] &&
      instant(platform.period_start) >= instant(platform.period_end)
    )
      errors['platform.period_end'] = 'Period end must be after period start.';
  }
  if (budget.default_policy) {
    const policy = budget.default_policy;
    for (const key of Object.keys(ownerAmounts) as (keyof typeof ownerAmounts)[])
      integer(
        `default_policy.${key}`,
        policy[key],
        1,
        key === 'max_calls_per_turn'
          ? 4_294_967_295
          : key === 'usage_timeout_seconds'
            ? 300
            : Number.MAX_SAFE_INTEGER,
      );
    if (
      safe(policy.turn_limit) &&
      safe(policy.owner_day_limit) &&
      policy.turn_limit > policy.owner_day_limit
    )
      errors['default_policy.turn_limit'] = 'The turn cap must not exceed the daily cap.';
    if (
      safe(policy.max_unobserved_units) &&
      safe(policy.turn_limit) &&
      policy.max_unobserved_units >= policy.turn_limit
    )
      errors['default_policy.max_unobserved_units'] =
        'Unobserved allowance must be below the turn cap.';
    if (!timezone(policy.day_timezone))
      errors['default_policy.day_timezone'] = 'Enter a valid IANA timezone name.';
  }
  return errors;
}
