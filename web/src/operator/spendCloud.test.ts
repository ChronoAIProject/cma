import { describe, expect, it } from 'vitest';
import {
  blankCloud,
  exactAmount,
  reviewSpendBudget,
  spendBudget,
  type CloudBudget,
} from './spendBudget';
import { spendAvailability } from './spendAvailability';
import { validateSpendBudget } from './spendValidation';
import { testBudget } from './spendTestFixtures';

const cloud: CloudBudget = { run_rate_units_per_day: 12, max_observation_age_seconds: 60 };
const budget = (value: CloudBudget | null | undefined = cloud) => ({
  ...testBudget,
  platform: { ...testBudget.platform!, cloud: value },
});

describe('cloud policy', () => {
  it.each([1, Number.MAX_SAFE_INTEGER])('accepts the exact inclusive boundary %s', (value) => {
    expect(
      validateSpendBudget(
        budget({
          run_rate_units_per_day: exactAmount(String(value)),
          max_observation_age_seconds: exactAmount(String(value)),
        }),
      ),
    ).toEqual({});
  });

  for (const key of ['run_rate_units_per_day', 'max_observation_age_seconds'] as const) {
    it.each(['', '0', '-1', '1.5', '9007199254740991.1', '9007199254740992', '1e2', ' 1', '01'])(
      `rejects ${key} input %j without rounding`,
      (text) => {
        const value = exactAmount(text);
        expect(validateSpendBudget(budget({ ...cloud, [key]: value }))).toHaveProperty([
          `platform.cloud.${key}`,
        ]);
        expect(value).toBe(text === '0' ? 0 : text);
      },
    );
  }

  it('requires both fields when configured and accepts an absent pair', () => {
    expect(blankCloud()).toEqual({ run_rate_units_per_day: '', max_observation_age_seconds: '' });
    expect(Object.keys(validateSpendBudget(budget(blankCloud())))).toEqual([
      'platform.cloud.run_rate_units_per_day',
      'platform.cloud.max_observation_age_seconds',
    ]);
    expect(validateSpendBudget(budget(null))).toEqual({});
    expect(validateSpendBudget(testBudget)).toEqual({});
  });

  it.each([
    false,
    [],
    {},
    { run_rate_units_per_day: 1 },
    { ...cloud, max_observation_age_seconds: false },
  ])('leaves an unsupported cloud shape opaque: %j', (value) => {
    expect(
      spendBudget({ ...testBudget, platform: { ...testBudget.platform, cloud: value } }),
    ).toBeUndefined();
  });

  it('preserves current unknown cloud data and unedited values during conflict review', () => {
    const base = budget({ ...cloud, future: { version: 1 } } as CloudBudget);
    const edited = budget({ ...base.platform.cloud!, run_rate_units_per_day: 24 });
    const latest = budget({
      ...cloud,
      max_observation_age_seconds: 90,
      future: { version: 2 },
    } as CloudBudget);
    expect(reviewSpendBudget(base, edited, latest)).toEqual({
      ...latest,
      platform: {
        ...latest.platform,
        cloud: { ...latest.platform.cloud, run_rate_units_per_day: 24 },
      },
    });
    const unrelated = { ...base, platform: { ...base.platform, warning_percent: 50 } };
    expect(reviewSpendBudget(base, unrelated, latest)).toEqual({
      ...latest,
      platform: { ...latest.platform, warning_percent: 50 },
    });
  });

  it('requires the unedited field again after a concurrent cloud removal', () => {
    const edited = budget({ ...cloud, run_rate_units_per_day: 24 });
    const result = spendBudget(reviewSpendBudget(budget(), edited, testBudget))!;
    expect(result.platform!.cloud).toEqual({ ...blankCloud(), run_rate_units_per_day: 24 });
    expect(validateSpendBudget(result)).toHaveProperty([
      'platform.cloud.max_observation_age_seconds',
    ]);
    const removed = { ...testBudget, platform: { ...testBudget.platform!, cloud: undefined } };
    expect(
      spendBudget(reviewSpendBudget(budget(), removed, budget()))!.platform!.cloud,
    ).toBeUndefined();
  });

  for (const enabled of [false, true]) {
    it.each([false, undefined])(
      `requires explicit platform availability with enabled=${enabled}: %s`,
      (available) => {
        expect(
          spendAvailability(
            {
              model_budget_writes_available: true,
              model_request_budget_writes_available: true,
              platform_spend_writes_available: available,
            },
            { ...budget(), enabled },
          ),
        ).toEqual({ edit: false, save: false });
      },
    );
  }

  it('requires each applicable gate without blocking policies that have no cloud', () => {
    expect(spendAvailability({ model_budget_writes_available: true }, testBudget)).toEqual({
      edit: true,
      save: true,
    });
    expect(spendAvailability({ platform_spend_writes_available: true }, budget())).toEqual({
      edit: false,
      save: false,
    });
    expect(
      spendAvailability(
        { model_budget_writes_available: true, platform_spend_writes_available: true },
        budget(),
      ),
    ).toEqual({ edit: true, save: true });
    expect(
      spendAvailability(
        { model_budget_writes_available: true, platform_spend_writes_available: true },
        {
          ...budget(),
          request_models: [{ model: 'fixture-model', input_overhead_tokens: 1 }],
        },
      ),
    ).toEqual({ edit: false, save: false });
    expect(spendAvailability({}, { enabled: false, owners: [] })).toEqual({
      edit: false,
      save: true,
    });
  });
});
