import {randomUUID} from 'node:crypto';
import {Blocked} from '../../runner.mjs';
import {writableSettings} from './public-setups.mjs';
import {withholdSettingsResponse} from './settings-response-fault.mjs';

async function disabledSettings(ctx) {
  const current = await writableSettings(ctx, true), s = current.settings;
  if (s.inactivity?.auto_pause_enabled !== false || s.triggers?.enabled !== false
    || s.model_budget?.enabled !== false || !Array.isArray(s.model_budget.owners)) {
    throw new Blocked('admin_conflict_requires_disabled_background_policies');
  }
  if (s.model_budget.platform?.cloud && current.platform_spend_writes_available !== true) {
    throw new Blocked('admin_conflict_cloud_writer_unavailable');
  }
  return s;
}

export async function settingsConflictSetup(ctx) {
  await disabledSettings(ctx);
  return {
    faults: {withholdNextSuccessfulResponse: options => withholdSettingsResponse(ctx, options)},
    async concurrentLists(currentCtx) {
      const settings = await disabledSettings(currentCtx);
      // Valid BudgetPolicy inputs. Disabled enforcement means synthetic owner
      // identities create no spend work; the case owns all writes/restoration.
      const policy = {owner_day_limit: 1000, turn_limit: 100, max_calls_per_turn: 10,
        day_timezone: 'Asia/Singapore', input_token_units: 1, output_token_units: 1,
        max_unobserved_units: 10, usage_timeout_seconds: 30};
      const budget = () => ({...structuredClone(settings.model_budget),
        owners: [{owner: 'c275-conflict-' + randomUUID(), policy: {...policy}}]});
      return {initial: budget(), server: budget(), draft: budget(), cleanup: async () => {}};
    },
    cleanup: async () => {},
  };
}
