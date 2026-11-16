import test from 'node:test';
import assert from 'node:assert/strict';
import {assessDefaults, assessRefusal} from './policy.mjs';

const defaults = () => ({schema: 'cma-cost-defaults-v1', scope: 'TEST_INPUT_ONLY',
  platform_limits: {max_sandboxes_per_user: 2}, sandbox_owner_limit: 2,
  model_budget: {enabled: true, default_policy: {owner_day_limit: 500, day_timezone: 'UTC'}}});
const valuation = {currency: 'USD', units_per_usd: 100, approved_rate_sha256: 'a'.repeat(64)};
const refusal = code => ({status: 429, body: {status: 429, type: 'urn:fixture:quota', code}});

test('default policy assessment requires two Sandboxes and an enabled USD 5 UTC allowance', () => {
  assert.equal(assessDefaults(defaults(), valuation).status, 'PASS');
  const actual = defaults(); actual.platform_limits.max_sandboxes_per_user = 10;
  actual.sandbox_owner_limit = 10; actual.model_budget = {enabled: false, owners: []};
  assert.equal(assessDefaults(actual, valuation).status, 'FAIL');
  assert.ok(assessDefaults(actual, valuation).checks.every(row => row.status === 'FAIL'));
});
test('a numeric limit alone never proves dollars, UTC-day policy or runtime qualification', () => {
  assert.equal(assessDefaults(defaults()).status, 'BLOCKED');
  assert.equal(assessDefaults(defaults(), {...valuation, approved_rate_sha256: ''}).status, 'BLOCKED');
  const wrong = defaults(); wrong.model_budget.default_policy.day_timezone = 'Asia/Singapore';
  assert.equal(assessDefaults(wrong, valuation).status, 'FAIL');
  wrong.model_budget.default_policy.day_timezone = 'UTC'; wrong.model_budget.default_policy.owner_day_limit = 501;
  assert.equal(assessDefaults(wrong, valuation).status, 'FAIL');
  assert.equal(assessDefaults(defaults(), valuation).runtime_acceptance, false);
});
test('typed 402, unrelated throttling and untyped 429 do not satisfy the required refusal', () => {
  assert.equal(assessRefusal(refusal('owner_budget_exhausted'), 'owner_budget_exhausted'), true);
  assert.equal(assessRefusal({status: 402, body: {...refusal('owner_budget_exhausted').body, status: 402}}, 'owner_budget_exhausted'), false);
  assert.equal(assessRefusal(refusal('rate_limited'), 'owner_budget_exhausted'), false);
  assert.equal(assessRefusal({status: 429, body: {}}, 'owner_budget_exhausted'), false);
});
test('missing fields, unsafe integers and unknown observation schemas cannot pass', () => {
  assert.throws(() => assessDefaults({}), /cost_defaults_schema/);
  const absent = defaults(); delete absent.platform_limits;
  assert.equal(assessDefaults(absent, valuation).status, 'FAIL');
  const unsafe = defaults(); unsafe.model_budget.default_policy.owner_day_limit = 2 ** 53;
  assert.equal(assessDefaults(unsafe, valuation).status, 'FAIL');
  assert.equal(assessDefaults(defaults(), {...valuation, units_per_usd: 0}).status, 'BLOCKED');
  assert.equal(assessDefaults(defaults(), {...valuation, units_per_usd: 2 ** 53}).status, 'BLOCKED');
});
