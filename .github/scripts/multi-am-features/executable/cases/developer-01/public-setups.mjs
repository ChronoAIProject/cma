import {Blocked} from '../../runner.mjs';
import {read, P, check} from './common.mjs';

// These recipes only read public settings. The case performs the UI mutation
// under the parent's lease and restores the actual original revisioned policy.
export async function writableSettings(ctx, requireBudgetWriter = false) {
  const current = await read(ctx, P + '/settings');
  check(Number.isSafeInteger(current.revision) && current.settings?.retention,
    'admin_settings_fixture_read');
  const budget = current.settings.model_budget;
  const hasModels = budget?.request_models?.length > 0;
  const retained = budget && (budget.enabled || budget.platform != null
    || budget.default_policy != null || budget.owners?.length > 0 || hasModels);
  // Disabled retained policies still require compatible writers on full PUTs.
  if ((requireBudgetWriter || retained) && current.model_budget_writes_available !== true) {
    throw new Blocked('admin_settings_spend_gate_closed');
  }
  if (hasModels && current.model_request_budget_writes_available !== true) {
    throw new Blocked('admin_settings_request_budget_gate_closed');
  }
  return current;
}

export const publicSetups = Object.freeze({
  async settings_retention(ctx) {
    await writableSettings(ctx);
    return {cleanup: async () => {}};
  },
  async spend_period(ctx) {
    const current = await writableSettings(ctx, true);
    const budget = current.settings.model_budget;
    if (!budget || typeof budget.enabled !== 'boolean' || !Array.isArray(budget.owners)) {
      throw new Blocked('admin_spend_schema_unavailable');
    }
    const now = new Date();
    const start = Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate());
    const period = {
      period_cap: Number.MAX_SAFE_INTEGER,
      period_start: new Date(start).toISOString(), period_end: new Date(start + 86400000).toISOString(),
      warning_percent: 60, stop_percent: 80, protected_reserve: 100, lag_allowance: 10,
    };
    if (budget.platform?.period_cap === period.period_cap) period.period_cap--;
    return {period, invalidPeriods: [
      {...period, period_end: period.period_start},
      {...period, warning_percent: period.stop_percent},
      {...period, period_start: '2026-02-30T00:00:00Z'},
      {...period, period_cap: '9007199254740992'},
      {...period, period_cap: '1.5'},
    ], cleanup: async () => {}};
  },
});
