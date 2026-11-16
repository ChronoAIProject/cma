import {define, setup, browser, evidence, settingsSession, navigateSettings, saveSettings,
  required, read, P, other, equal, sameSettings} from './common.mjs';
import {spendPeriodScenario} from './spend-period.mjs';
import {availabilityScenario} from './spend-availability.mjs';
import {defaultOwnerPolicyScenario} from './default-owner-policy.mjs';

const owner = {
  owner_day_limit: 'Per-owner daily cap (accounting units)', turn_limit: 'Per-turn cap (accounting units)',
  max_calls_per_turn: 'Maximum model calls per turn', day_timezone: 'Day-boundary timezone (IANA)',
  input_token_units: 'Input token valuation (accounting units per token)',
  output_token_units: 'Output token valuation (accounting units per token)',
  max_unobserved_units: 'Unobserved turn allowance (accounting units)',
  usage_timeout_seconds: 'Usage reporting timeout (seconds)',
};

export async function fillFields(page, labels, value, prefix = '') {
  for (const [key, label] of Object.entries(labels)) {
    required(value[key] !== undefined, 'admin_spend_field_setup_missing');
    await page.getByLabel(prefix + label, {exact: true}).fill(String(value[key]));
  }
}

export const spendAvailability = define(53, 'browser', 2, ctx => availabilityScenario(ctx, owner), ['admin-fixtures', 'browser']);

export const spendPeriod = define(54, 'browser', 2, spendPeriodScenario, ['admin-fixtures', 'browser']);

export const defaultOwnerPolicy = define(55, 'browser', 2, defaultOwnerPolicyScenario, ['admin-fixtures', 'browser']);

export {owner as ownerFields};
