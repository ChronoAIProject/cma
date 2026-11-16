import {required} from './common.mjs';
import {cloudTime} from './cloud-observation-data.mjs';

export const periodFields = {
  period_cap: 'Platform period cap (accounting units)', warning_percent: 'Warning threshold (%)',
  stop_percent: 'Hard-stop threshold (%)', protected_reserve: 'Protected reserve for existing users (accounting units)',
  lag_allowance: 'Unmetered-lag allowance (accounting units)',
  period_start: 'Period start (inclusive, RFC3339)', period_end: 'Period end (exclusive, RFC3339)',
};

export function requirePeriod(period) {
  required(period && Object.keys(periodFields).every(k => Object.hasOwn(period, k))
    && Object.keys(period).every(k => Object.hasOwn(periodFields, k)), 'admin_period_fixture_shape');
  required([Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER - 1].includes(period.period_cap)
    && ['warning_percent', 'stop_percent'].every(k => Number.isSafeInteger(period[k]) && period[k] >= 1 && period[k] <= 100)
    && period.warning_percent < period.stop_percent
    && ['protected_reserve', 'lag_allowance'].every(k => Number.isSafeInteger(period[k]) && period[k] >= 0),
  'admin_period_exact_integer_fixture');
  required(BigInt(period.protected_reserve) + BigInt(period.lag_allowance)
    < BigInt(period.period_cap) * BigInt(period.stop_percent) / 100n, 'admin_period_reserve_fixture');
  required(['period_start', 'period_end'].every(k => Number.isFinite(cloudTime(period[k])))
    && cloudTime(period.period_start) < cloudTime(period.period_end),
  'admin_period_time_fixture');
}

export function invalidPlatformPeriods(period) {
  const integer = (min = 1, max = Number.MAX_SAFE_INTEGER) =>
    `Enter a whole number from ${min} to ${max}, without decimals or exponents.`;
  const cases = [
    ...['9007199254740992', '1.5', '1e3', '01', '-1', 0].map(value => ['period_cap', {period_cap: value}, integer()]),
    ['warning_percent', {warning_percent: 0}, integer(1, 100)],
    ['stop_percent', {stop_percent: 101}, integer(1, 100)],
    ['lag_allowance', {lag_allowance: -1}, integer(0)],
    ['warning_percent', {warning_percent: period.stop_percent}, 'Warning must be below the hard-stop percentage.'],
    ['protected_reserve', {period_cap: 100, protected_reserve: 80, lag_allowance: 0, warning_percent: 60, stop_percent: 80},
      'Reserve plus lag allowance must be below the hard-stop threshold.'],
    ['period_end', {period_end: period.period_start}, 'Period end must be after period start.'],
    ['period_end', {period_end: new Date(Date.parse(period.period_start) - 1000).toISOString().replace('.000Z', 'Z')},
      'Period end must be after period start.'],
    ...['2026-02-30T00:00:00Z', '2026-11-17T00:00:00'].map(value => ['period_start', {period_start: value},
      'Enter a valid RFC3339 instant with an explicit timezone offset.']),
  ];
  return cases.map(([field, patch, error]) => ({field, error, value: {...period, ...patch}}));
}
