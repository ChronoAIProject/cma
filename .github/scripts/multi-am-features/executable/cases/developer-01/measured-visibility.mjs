import {setup, settingsSession, read, P, sameSettings, required, equal} from './common.mjs';
import {measuredFixture} from './measured-proof.mjs';
import {measuredSettingsUI} from './measured-settings-ui.mjs';
import {measuredReport} from './measured-report.mjs';

export async function measuredVisibilityScenario(ctx) {
  const f = await setup(ctx, 'measured_visibility'); measuredFixture(f);
  const state = await settingsSession(ctx, f);
  required(typeof state.original.settings.usage_reporting?.measured_visible === 'boolean', 'admin_measured_original_policy');
  const ui = await measuredSettingsUI(ctx, state), reports = [];
  let finalityJob;
  const finalize = () => finalityJob ??= (async () => {
    const errors = [];
    try {await ui.close();} catch (error) {errors.push(error);}
    try {await state.restore();} catch (error) {errors.push(error);}
    if (errors.length) throw new AggregateError(errors, 'admin_measured_finality');
  })();
  // Report close hooks register after this finalizer: reverse cleanup drains
  // every report listener/browser before the settings browser and restoration.
  ctx.defer('admin_measured_finalize', finalize);
  await ctx.step('step_1', async () => {
    for (const enabled of [!state.original.settings.usage_reporting.measured_visible,
      state.original.settings.usage_reporting.measured_visible]) {
      await ui.change(enabled);
      for (const actor of ['user', 'admin']) reports.push(await measuredReport(ctx, f, actor, enabled));
    }
  });
  await ctx.step('step_2', async () => {
    await finalize();
    for (const am of ['a', 'b']) ctx.assert(sameSettings((await read(ctx, P + '/settings', am)).settings,
      state.original.settings), 'admin_measured_restore');
  });
  await ctx.step('expected_result', async () => {
    for (const actor of ['user', 'admin']) {
      const pair = reports.filter(r => r.actor === actor);
      ctx.assert(pair.length === 2 && pair[0].enabled !== pair[1].enabled, 'admin_measured_toggle_cohort');
      ctx.assert(equal(pair[0].report.summary.allocated_period, pair[1].report.summary.allocated_period)
        && equal(pair[0].report.summary.total_allocated_capacity, pair[1].report.summary.total_allocated_capacity),
      'admin_measured_toggle_changed_allocations');
    }
  });
}
