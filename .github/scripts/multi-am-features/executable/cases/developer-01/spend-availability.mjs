import {setup, settingsSession, request, P, other, equal, sameSettings, required, check} from './common.mjs';
import {availabilityUI} from './spend-availability-ui.mjs';
import {cloudTime} from './cloud-observation-data.mjs';

const flags = ['model_budget_writes_available', 'model_request_budget_writes_available', 'platform_spend_writes_available'];

export async function availabilityScenario(ctx, ownerFields) {
  const f = await setup(ctx, 'spend_availability');
  required(typeof f.setWriterAvailability === 'function', 'admin_spend_writer_fixture_missing');
  const state = await settingsSession(ctx, f), initial = state.original, budget = initial.settings.model_budget;
  required(flags.every(k => typeof initial[k] === 'boolean') && initial.model_budget_writes_available === true
    && initial.model_request_budget_writes_available === true, 'admin_spend_availability_writable_fixture');
  required(budget && typeof budget.enabled === 'boolean' && budget.platform
    && Number.isFinite(cloudTime(budget.platform.period_start)) && Number.isFinite(cloudTime(budget.platform.period_end))
    && cloudTime(budget.platform.period_start) < cloudTime(budget.platform.period_end)
    && Array.isArray(budget.owners) && budget.owners.length <= 1024
    && (budget.default_policy || budget.owners.length > 0)
    && (!budget.request_models || Array.isArray(budget.request_models) && budget.request_models.length <= 64)
    && (!budget.platform.cloud || initial.platform_spend_writes_available), 'admin_spend_availability_retained_fixture');
  let ui, modeIntent = false, recoveryJob, finalityJob, saved;
  const snapshot = async (am, label) => {
    const response = await request(ctx, 'GET', P + '/settings', undefined, {am});
    await ctx.observe(label, async () => ({am, response}));
    check(response.status === 200 && Number.isSafeInteger(response.body?.revision) && response.body.settings,
      'admin_spend_availability_snapshot');
    return response.body;
  };
  const pair = async (expected, label) => {
    for (const am of [ctx.am, other(ctx.am)]) check(equal(await snapshot(am, label), expected), 'admin_spend_availability_peer');
  };
  // This same promise serves normal recovery and cleanup. An uncertain mode
  // change is observed, never dispatched a second time through another hook.
  const recover = () => recoveryJob ??= Promise.resolve().then(() => f.setWriterAvailability(initial.model_budget_writes_available));
  const finalize = () => finalityJob ??= (async () => {
    const errors = [];
    if (ui) try {await ui.close();} catch (e) {errors.push(e);}
    if (modeIntent) try {await recover();} catch (e) {errors.push(e);}
    try {await state.restore();} catch (e) {errors.push(e);}
    const final = [];
    for (const am of ['a', 'b']) try {
      const value = await snapshot(am, 'admin_spend_availability_final'); final.push(value);
      check(value.revision >= initial.revision && sameSettings(value.settings, initial.settings)
        && flags.every(k => value[k] === initial[k]), 'admin_spend_availability_restored');
    } catch (e) {errors.push(e);}
    if (final.length === 2 && !equal(final[0], final[1])) errors.push(Error('admin_spend_availability_final_peer'));
    if (errors.length) throw new AggregateError(errors, 'admin_spend_availability_finality');
  })();
  ctx.defer('admin_spend_availability_finalize', finalize);
  ui = await availabilityUI(ctx, ownerFields);
  const desired = structuredClone(initial.settings); desired.model_budget.enabled = !budget.enabled;
  // The public request/cloud flags each include the foundation writer gate.
  const unavailable = {...initial, ...Object.fromEntries(flags.map(key => [key, false]))};
  await ctx.step('step_1', async () => {
    await pair(initial, 'admin_spend_availability_initial'); await ui.open(budget);
    await ui.toggle(desired.model_budget.enabled);
    modeIntent = true; await f.setWriterAvailability(false);
    await pair(unavailable, 'admin_spend_availability_closed');
    state.own(desired);
    await ui.save({expected_revision: initial.revision, settings: desired}, 503);
    await ui.refused(desired.model_budget);
    await pair(unavailable, 'admin_spend_availability_no_effect');
    await recover(); await pair(initial, 'admin_spend_availability_recovered');
    await ui.refresh(initial, desired.model_budget);
  });
  await ctx.step('step_2', async () => {
    state.own(desired);
    saved = await ui.save({expected_revision: initial.revision, settings: desired}, 200);
    ctx.assert(Number.isSafeInteger(saved.body.revision) && saved.body.revision > initial.revision
      && sameSettings(saved.body.settings, desired) && flags.every(k => saved.body[k] === initial[k]),
    'admin_spend_availability_saved_policy');
    await ui.saved(desired.model_budget); await pair(saved.body, 'admin_spend_availability_saved');
    await finalize();
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(saved.body.settings.model_budget.enabled === !budget.enabled, 'admin_spend_availability_changed');
  });
}
