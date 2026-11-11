import { describe, expect, it } from 'vitest';
import { reviewSpendBudget, spendBudget, spendListConflict } from './spendBudget';
import { validateSpendBudget } from './spendValidation';
import { testBudget } from './spendTestFixtures';

const owner = { owner: 'fixture-owner', policy: testBudget.default_policy! };
const model = { model: 'fixture-model', input_overhead_tokens: 1 };
const base = { ...testBudget, owners: [owner], request_models: [model] };

describe('explicit spend policy lists', () => {
  it('validates an isolated owner policy without a default, including model configuration', () => {
    expect(validateSpendBudget({ ...base, enabled: true, default_policy: undefined })).toEqual({});
    expect(
      validateSpendBudget({ ...base, enabled: true, default_policy: undefined, owners: [] }),
    ).toHaveProperty('enabled');
  });
  it.each(['', ' fixture-owner', 'fixture-owner '])('rejects an invalid owner key %j', (key) => {
    expect(validateSpendBudget({ ...base, owners: [{ ...owner, owner: key }] })).toHaveProperty([
      'owners.0.owner',
    ]);
  });
  it('checks every override rather than only the default policy', () => {
    const errors = validateSpendBudget({
      ...base,
      owners: [
        owner,
        {
          owner: 'fixture-other',
          policy: {
            ...owner.policy,
            turn_limit: Number.MAX_SAFE_INTEGER,
            max_calls_per_turn: 0,
            day_timezone: 'Invalid/Zone',
          },
        },
      ],
    });
    expect(errors).toHaveProperty(['owners.1.policy.turn_limit']);
    expect(errors).toHaveProperty(['owners.1.policy.max_calls_per_turn']);
    expect(errors).toHaveProperty(['owners.1.policy.day_timezone']);
  });
  it('rejects duplicate owner and model keys and excessive list sizes', () => {
    expect(validateSpendBudget({ ...base, owners: [owner, owner] })).toHaveProperty([
      'owners.1.owner',
    ]);
    expect(validateSpendBudget({ ...base, request_models: [model, model] })).toHaveProperty([
      'request_models.1.model',
    ]);
    expect(validateSpendBudget({ ...base, owners: Array(1025).fill(owner) })).toHaveProperty(
      'owners',
    );
    expect(validateSpendBudget({ ...base, request_models: Array(65).fill(model) })).toHaveProperty(
      'request_models',
    );
  });
  it.each(['', ' fixture-model', 'fixture-model ', 'é'.repeat(65)])(
    'rejects invalid model name %j',
    (name) => {
      expect(
        validateSpendBudget({ ...base, request_models: [{ ...model, model: name }] }),
      ).toHaveProperty(['request_models.0.model']);
    },
  );
  it.each([0, 1_048_577, '1e2', '1.1', Number.MAX_SAFE_INTEGER + 1])(
    'rejects invalid framing overhead %j',
    (value) => {
      expect(
        validateSpendBudget({
          ...base,
          request_models: [{ ...model, input_overhead_tokens: value }],
        }),
      ).toHaveProperty(['request_models.0.input_overhead_tokens']);
    },
  );
  it.each([null, {}, [null], [{ model: 'fixture-model' }]])(
    'refuses an unsupported request-model shape %j',
    (request_models) => {
      expect(spendBudget({ ...base, request_models })).toBeUndefined();
    },
  );
  it('preserves model/owner edits on revision review when the server edited other fields', () => {
    const edited = {
      ...base,
      owners: [{ ...owner, policy: { ...owner.policy, max_calls_per_turn: 2 } }],
      request_models: [],
    };
    const latest = {
      ...base,
      platform: { ...base.platform!, warning_percent: 50 },
      future: 'retained',
    };
    expect(spendListConflict(base, edited, latest)).toBe(false);
    expect(reviewSpendBudget(base, edited, latest)).toEqual({
      ...latest,
      owners: edited.owners,
      request_models: [],
    });
  });
  it.each(['owners', 'request_models'] as const)(
    'requires a fresh list review when both sides change %s',
    (key) => {
      const edited = { ...base, [key]: [] };
      const latest = {
        ...base,
        [key]:
          key === 'owners'
            ? [{ ...owner, policy: { ...owner.policy, max_calls_per_turn: 2 } }]
            : [{ ...model, input_overhead_tokens: 2 }],
      };
      expect(spendListConflict(base, edited, latest)).toBe(true);
      expect(spendListConflict(base, edited, edited)).toBe(false);
      expect(spendListConflict(base, base, latest)).toBe(false);
    },
  );
});
