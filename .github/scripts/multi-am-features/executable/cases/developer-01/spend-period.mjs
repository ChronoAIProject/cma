import {setup, settingsSession, read, P, sameSettings, required} from './common.mjs';
import {requirePeriod, invalidPlatformPeriods} from './spend-period-data.mjs';
import {spendPeriodUI} from './spend-period-ui.mjs';

export async function spendPeriodScenario(ctx) {
  const fixture = await setup(ctx, 'spend_period'); requirePeriod(fixture.period);
  const state = await settingsSession(ctx, fixture);
  required(state.original.model_budget_writes_available === true && state.original.settings.model_budget,
    'admin_period_writer_fixture');
  const ui = await spendPeriodUI(ctx, state);
  let finalityJob, changed;
  const finalize = () => finalityJob ??= (async () => {
    const errors = [];
    try {await ui.close();} catch (error) {errors.push(error);}
    try {await state.restore();} catch (error) {errors.push(error);}
    if (errors.length) throw new AggregateError(errors, 'admin_period_finality');
  })();
  ctx.defer('admin_period_finalize', finalize);
  await ctx.step('step_1', async () => {
    await ui.open();
    for (const invalid of invalidPlatformPeriods(fixture.period)) await ui.invalid(invalid);
  });
  await ctx.step('step_2', async () => {
    changed = await ui.save(fixture.period);
    await finalize();
    for (const am of ['a', 'b']) ctx.assert(sameSettings((await read(ctx, P + '/settings', am)).settings,
      state.original.settings), 'admin_period_restored');
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(Object.keys(fixture.period).every(k => changed.settings.model_budget.platform[k] === fixture.period[k]),
      'admin_period_exact_values');
  });
}
