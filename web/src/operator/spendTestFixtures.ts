import type { SpendBudget } from './spendBudget';

// Synthetic accounting values only, never deployment policy or monetary defaults.
export const testBudget: SpendBudget = {
  enabled: false,
  platform: {
    period_cap: 10000,
    period_start: '2026-11-01T00:00:00Z',
    period_end: '2026-12-01T00:00:00Z',
    warning_percent: 60,
    stop_percent: 80,
    protected_reserve: 100,
    lag_allowance: 10,
  },
  default_policy: {
    owner_day_limit: 1000,
    turn_limit: 100,
    max_calls_per_turn: 3,
    day_timezone: 'Asia/Singapore',
    input_token_units: 1,
    output_token_units: 2,
    max_unobserved_units: 10,
    usage_timeout_seconds: 30,
  },
  owners: [],
};
