import {setup, settingsSession, request, read, P, other, required, equal, sameSettings} from './common.mjs';
import {retentionBrowser} from './retention-browser.mjs';

export async function retentionScenario(ctx) {
  const f = await setup(ctx, 'settings_retention'), state = await settingsSession(ctx, f);
  const originalDays = state.original.settings.retention?.agent_log_days;
  required(Number.isInteger(originalDays) && originalDays >= 0 && originalDays <= 4294967295,
    'admin_retention_original_unsigned');
  const ui = await retentionBrowser(ctx, state);
  let changed, finalityJob;
  const finalize = () => finalityJob ??= (async () => {
    const errors = [];
    try {await ui.close();} catch (error) {errors.push(error);}
    try {await state.restore();} catch (error) {errors.push(error);}
    if (errors.length) throw new AggregateError(errors, 'admin_retention_finality');
  })();
  ctx.defer('admin_retention_finalize', finalize);
  await ctx.step('step_1', async () => {
    changed = await ui.save(originalDays === 30 ? 31 : 30);
    for (const input of ['-1', '1.5', '4294967296', '']) await ui.invalid(input, changed);
    await ui.close();
    for (const agent_log_days of [-1, 1.5, 4294967296, 'not-a-number']) {
      const before = await read(ctx, P + '/settings');
      ctx.assert(equal(before, changed), 'admin_retention_negative_baseline');
      const retention = {...before.settings.retention, agent_log_days};
      state.own({...before.settings, retention});
      const refusal = await request(ctx, 'PATCH', P + '/settings', {expected_revision: before.revision, retention}, {allowed: [422]});
      ctx.assert(refusal.status === 422 && refusal.body?.code === 'invalid_request', 'admin_retention_invalid_refusal');
      for (const am of [ctx.am, other(ctx.am)]) ctx.assert(equal(changed, await read(ctx, P + '/settings', am)),
        'admin_retention_invalid_api_effect');
    }
  });
  await ctx.step('step_2', async () => {
    for (const am of ['a', 'b']) ctx.assert(equal(changed, await read(ctx, P + '/settings', am)), 'admin_retention_peer');
    await finalize();
    for (const am of ['a', 'b']) ctx.assert(sameSettings((await read(ctx, P + '/settings', am)).settings,
      state.original.settings), 'admin_retention_restored');
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(sameSettings(changed.settings, {...state.original.settings, retention: changed.settings.retention}),
      'admin_retention_other_settings');
  });
}
