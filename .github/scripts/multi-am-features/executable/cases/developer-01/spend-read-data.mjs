import {check, required, equal} from './common.mjs';
import {validateObservationSpend, cloudTime} from './cloud-observation-data.mjs';

const unsigned = value => Number.isSafeInteger(value) && value >= 0;
export const withoutRevision = value => {const {revision, ...rest} = value; return rest;};

export function requireSpendModes(f) {
  required(f.exclusiveSettings === true && typeof f.selectMode === 'function' && typeof f.restoreMode === 'function',
    'admin_spend_modes_missing');
  required(validateObservationSpend(f.expectedEnabled) && f.expectedEnabled.metering_enabled
    && f.expectedEnabled.ledger?.cloud && f.expectedEnabled.committed > 0,
  'admin_spend_enabled_inputs_missing');
  required(validateObservationSpend(f.expectedDisabled) && f.expectedDisabled.metering_enabled === false,
    'admin_spend_disabled_inputs_missing');
  required(validateObservationSpend(f.expectedWithoutCloud) && f.expectedWithoutCloud.metering_enabled
    && f.expectedWithoutCloud.ledger && !Object.hasOwn(f.expectedWithoutCloud.ledger, 'cloud')
    && !Object.hasOwn(f.expectedWithoutCloud, 'committed'), 'admin_spend_missing_cloud_inputs_missing');
}

export function verifySpendRead(state, settings, expected) {
  check(validateObservationSpend(state), 'admin_spend_read_shape');
  const budget = settings?.settings?.model_budget, platform = budget?.platform;
  check(unsigned(settings?.revision) && typeof budget?.enabled === 'boolean', 'admin_spend_settings_shape');
  const enabled = budget.enabled && platform?.cloud != null;
  check(state.metering_enabled === enabled && state.settings_revision === settings.revision, 'admin_spend_mode_revision');
  if (enabled) {
    check(['period_cap', 'warning_percent', 'stop_percent', 'protected_reserve', 'lag_allowance']
      .every(k => unsigned(platform[k])) && platform.period_cap > 0
      && platform.warning_percent > 0 && platform.warning_percent < platform.stop_percent
      && platform.stop_percent <= 100 && unsigned(platform.cloud.run_rate_units_per_day)
      && platform.cloud.run_rate_units_per_day > 0 && unsigned(platform.cloud.max_observation_age_seconds)
      && platform.cloud.max_observation_age_seconds > 0
      && Number.isFinite(cloudTime(platform.period_start)) && Number.isFinite(cloudTime(platform.period_end))
      && cloudTime(platform.period_start) < cloudTime(platform.period_end), 'admin_spend_policy_shape');
    const level = percent => BigInt(platform.period_cap) * BigInt(percent) / 100n;
    const disposable = level(platform.stop_percent) - BigInt(platform.protected_reserve) - BigInt(platform.lag_allowance);
    check(disposable > 0n && equal(state.levels, {warning: Number(level(platform.warning_percent)),
      stop: Number(level(platform.stop_percent)), disposable: Number(disposable)}), 'admin_spend_levels_mismatch');
    if (state.ledger) check(state.ledger.period_start === platform.period_start && state.ledger.period_end === platform.period_end,
      'admin_spend_period_mismatch');
    if (!state.ledger?.cloud) check(!Object.hasOwn(state, 'committed'), 'admin_missing_cloud_fabricated_zero');
  }
  if (expected) check(equal(state, expected), 'admin_spend_accounting_mismatch');
}

export function verifySpendRefusal(response, status, code) {
  check(response.status === status && response.body?.code === code, 'admin_spend_wrong_refusal');
  check(['levels', 'committed', 'ledger', 'settings_revision', 'metering_enabled', 'cloud', 'units']
    .every(k => !Object.hasOwn(response.body, k)), 'admin_spend_refusal_exposed_accounting');
}
