import {define, setup, browser, evidence, settingsSession, navigateSettings, saveSettings,
  required, read, P, other, equal, sameSettings} from './common.mjs';
import {spendPeriodScenario} from './spend-period.mjs';
import {availabilityScenario} from './spend-availability.mjs';

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

export const defaultOwnerPolicy = define(55, 'browser', 2, async ctx => {
  const f = await setup(ctx, 'default_owner_policy'), state = await settingsSession(ctx, f);
  required(f.policy && typeof f.readReservations === 'function' && typeof f.admitModelRequest === 'function',
    'admin_default_budget_fixture_missing');
  const s = await browser(ctx); let reservations;
  await ctx.step('step_1', async () => {
    reservations = await ctx.observe('admin_reservations_before', () => f.readReservations(ctx));
    required(reservations.items?.length > 0, 'admin_existing_reservations_missing');
    await navigateSettings(s);
    if (await s.page.getByRole('button', {name: 'Configure default owner policy', exact: true}).isVisible()) {
      await s.page.getByRole('button', {name: 'Configure default owner policy', exact: true}).click();
    }
    await fillFields(s.page, owner, f.policy);
    const before = await read(ctx, P + '/settings');
    state.own({...before.settings, model_budget: {...before.settings.model_budget, default_policy: f.policy}});
    await saveSettings(s);
    const actual = await read(ctx, P + '/settings', other(ctx.am));
    ctx.assert(equal(actual.settings.model_budget.default_policy, f.policy), 'admin_default_owner_policy_readback');
    const applied = await ctx.observe('admin_default_policy_admission', () => f.admitModelRequest(ctx, 'without_override'));
    const override = await ctx.observe('admin_override_policy_admission', () => f.admitModelRequest(ctx, 'with_override'));
    ctx.assert(equal(applied.reservation?.policy, f.policy)
      && equal(override.reservation?.policy, f.overridePolicy), 'admin_default_policy_scope');
    const without = structuredClone(actual.settings); delete without.model_budget.default_policy; state.own(without);
    await s.page.getByRole('button', {name: 'Remove default owner policy', exact: true}).click(); await saveSettings(s);
    ctx.assert((await read(ctx, P + '/settings')).settings.model_budget.default_policy === undefined,
      'admin_default_policy_not_removed');
  });
  await ctx.step('step_2', async () => {
    const retained = await ctx.observe('admin_reservations_after', () => f.readReservations(ctx));
    ctx.assert(reservations.items.every(r => retained.items.some(v => equal(v, r))), 'admin_removal_erased_reservations');
    await state.restore();
    ctx.assert(sameSettings((await read(ctx, P + '/settings', other(ctx.am))).settings, state.original.settings),
      'admin_default_policy_restore');
  });
  await ctx.step('expected_result', async () => { await evidence(ctx, s); });
}, ['admin-fixtures', 'browser']);

export {owner as ownerFields};
