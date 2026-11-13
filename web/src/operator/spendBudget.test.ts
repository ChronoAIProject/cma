import { describe, expect, it } from 'vitest';
import {
  blankOwner,
  blankPlatform,
  exactAmount,
  reviewSpendBudget,
  spendBudget,
} from './spendBudget';
import { validateSpendBudget } from './spendValidation';
import { testBudget as budget } from './spendTestFixtures';

describe('spend policy validation', () => {
  it('accepts complete policies, explicit zero reserve and disabled empty configuration', () => {
    expect(validateSpendBudget(budget)).toEqual({});
    expect(
      validateSpendBudget({
        ...budget,
        enabled: true,
        platform: { ...budget.platform!, protected_reserve: 0, lag_allowance: 0 },
      }),
    ).toEqual({});
    expect(validateSpendBudget({ enabled: false, owners: [] })).toEqual({});
  });
  it.each(['', '1.5', '9007199254740991.1', '9007199254740992', '1e2', '-1', ' 1', '01'])(
    'preserves invalid integer text %j and rejects it before serialization',
    (text) => {
      const value = exactAmount(text);
      expect(value).toBe(text);
      expect(
        validateSpendBudget({
          ...budget,
          default_policy: { ...budget.default_policy!, turn_limit: value },
        }),
      ).toHaveProperty(['default_policy.turn_limit']);
    },
  );
  it('accepts the largest safe integer without floating-point threshold multiplication', () => {
    expect(exactAmount('9007199254740991')).toBe(Number.MAX_SAFE_INTEGER);
    const cap = Number.MAX_SAFE_INTEGER;
    const threshold = Number((BigInt(cap) * 80n) / 100n);
    expect(
      validateSpendBudget({
        ...budget,
        platform: {
          ...budget.platform!,
          period_cap: cap,
          protected_reserve: threshold - 1,
          lag_allowance: 0,
        },
      }),
    ).toEqual({});
    expect(
      validateSpendBudget({
        ...budget,
        platform: {
          ...budget.platform!,
          period_cap: cap,
          protected_reserve: threshold,
          lag_allowance: 0,
        },
      }),
    ).toHaveProperty(['platform.protected_reserve']);
  });
  it.each([
    ['warning_percent', 0],
    ['warning_percent', 80],
    ['stop_percent', 101],
    ['protected_reserve', -1],
    ['lag_allowance', -1],
    ['period_cap', 0],
    ['period_start', '2026-02-30T00:00:00Z'],
    ['period_start', '2026-11-01T00:00:00'],
    ['period_end', '2026-10-01T00:00:00Z'],
    ['period_end', 'invalid'],
  ])('rejects invalid platform %s', (key, value) => {
    expect(
      Object.keys(
        validateSpendBudget({ ...budget, platform: { ...budget.platform!, [key]: value } }),
      ),
    ).not.toHaveLength(0);
  });
  it.each([
    ['turn_limit', 1001],
    ['max_unobserved_units', 100],
    ['max_calls_per_turn', 0],
    ['max_calls_per_turn', 4294967296],
    ['usage_timeout_seconds', 301],
    ['input_token_units', 0],
    ['output_token_units', Infinity],
    ['day_timezone', 'Invalid/Zone'],
    ['day_timezone', '+01:00'],
  ])('rejects invalid owner %s', (key, value) => {
    expect(
      Object.keys(
        validateSpendBudget({
          ...budget,
          default_policy: { ...budget.default_policy!, [key]: value },
        }),
      ),
    ).not.toHaveLength(0);
  });
  it('requires complete policies on activation, and never creates default values', () => {
    expect(validateSpendBudget({ enabled: true, owners: [] })).toHaveProperty('enabled');
    expect(Object.values(blankPlatform())).toEqual(Array(7).fill(''));
    expect(Object.values(blankOwner())).toEqual(Array(8).fill(''));
    expect(
      Object.keys(
        validateSpendBudget({
          enabled: false,
          owners: [],
          platform: blankPlatform(),
          default_policy: blankOwner(),
        }),
      ),
    ).toHaveLength(15);
  });
  it('identifies missing and predecessor contracts as unavailable', () => {
    for (const value of [
      undefined,
      null,
      {},
      { owners: [] },
      { enabled: true, owners: {}, platform: {} },
    ])
      expect(spendBudget(value)).toBeUndefined();
    expect(spendBudget(budget)).toBe(budget);
  });
  it('keeps latest owner overrides, unedited values and unknown fields on explicit review', () => {
    const current = {
      ...budget,
      owners: [{ owner: 'fixture-owner', policy: budget.default_policy! }],
      platform: { ...budget.platform!, warning_percent: 50 },
      future: 'preserved',
    };
    const edited = { ...budget, default_policy: { ...budget.default_policy!, turn_limit: 80 } };
    expect(reviewSpendBudget(budget, edited, current)).toEqual({
      ...current,
      default_policy: edited.default_policy,
    });
    expect(reviewSpendBudget(budget, budget, current)).toBe(current);
    expect(reviewSpendBudget(budget, edited, undefined)).toBeUndefined();
  });
  it('does not resurrect other stale values when a concurrent editor removed a policy', () => {
    const result = spendBudget(
      reviewSpendBudget(
        budget,
        { ...budget, default_policy: { ...budget.default_policy!, turn_limit: 80 } },
        { ...budget, default_policy: undefined },
      ),
    )!;
    expect(result.default_policy).toEqual({ ...blankOwner(), turn_limit: 80 });
    expect(Object.keys(validateSpendBudget(result)).length).toBeGreaterThan(0);
  });
});
