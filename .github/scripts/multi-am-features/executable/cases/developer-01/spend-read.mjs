import {define, setup, request, P, equal, check} from './common.mjs';
import {durableSpend, validateObservationSpend} from './cloud-observation-data.mjs';
import {requireSpendModes, verifySpendRead, verifySpendRefusal, withoutRevision} from './spend-read-data.mjs';

export const spendRead = define(77, 'global', 2, async ctx => {
  const f = await setup(ctx, 'spend_read'); requireSpendModes(f);
  let original, modeIntent = false, finalityJob, finalized = false;
  const get = async (path, am, label, {actor = 'admin', allowed = [200]} = {}) => {
    const response = await request(ctx, 'GET', path, undefined, {am, actor, allowed});
    await ctx.observe(label, async () => ({am, response})); return response;
  };
  const pair = async (label, expected) => {
    const rows = [];
    for (const am of ['a', 'b']) {
      const settings = (await get(P + '/settings', am, label + '_settings')).body;
      const state = (await get(P + '/spend', am, label + '_spend')).body;
      verifySpendRead(state, settings, expected); rows.push({am, settings, state});
    }
    check(equal(rows[0].settings, rows[1].settings)
      && equal(durableSpend(rows[0].state), durableSpend(rows[1].state)), 'admin_spend_peer_mismatch');
    return rows[0];
  };
  const select = async mode => {
    await ctx.observe('admin_spend_mode_intent', async () => ({mode}));
    modeIntent = true; await f.selectMode(mode);
  };
  const unauthorized = async () => {
    for (const am of ['a', 'b']) verifySpendRefusal(await get(P + '/spend', am, 'admin_spend_unauthorized',
      {actor: 'user', allowed: [403]}), 403, 'forbidden');
  };
  const finalize = () => finalityJob ??= (async () => {
    if (!original) return;
    const errors = [], settingsRead = [];
    // Restore the parent's captured mode once. Its result is never a verdict:
    // the case independently reads policy, accounting and both serving AMs.
    if (modeIntent) try {await f.restoreMode();} catch (e) {errors.push(e);}
    for (const am of ['a', 'b']) {
      let settings;
      try {
        settings = (await get(P + '/settings', am, 'admin_spend_final_settings')).body;
        check(Number.isSafeInteger(settings?.revision) && settings.revision >= original.settings.revision,
          'admin_spend_restore_revision');
        check(equal(withoutRevision(settings), withoutRevision(original.settings)), 'admin_spend_restore_settings');
        settingsRead.push(settings);
      } catch (e) {errors.push(e);}
      try {
        const state = (await get(P + '/spend', am, 'admin_spend_final_spend')).body;
        check(validateObservationSpend(state), 'admin_spend_read_shape');
        if (settings) verifySpendRead(state, settings);
        check(equal(durableSpend(state), durableSpend({...original.state,
          settings_revision: settings?.revision ?? original.state.settings_revision})), 'admin_spend_restore_accounting');
      } catch (e) {errors.push(e);}
    }
    if (settingsRead.length === 2 && !equal(settingsRead[0], settingsRead[1])) errors.push(Error('admin_spend_restore_peer_mismatch'));
    if (errors.length) throw new AggregateError(errors, 'admin_spend_read_finality');
    finalized = true;
  })();
  ctx.defer('admin_spend_mode_finalize', finalize);
  await ctx.step('step_1', async () => {
    original = structuredClone(await pair('admin_spend_initial'));
    for (const [mode, expected] of [['enabled', f.expectedEnabled], ['disabled', f.expectedDisabled]]) {
      await select(mode); await pair('admin_spend_' + mode, expected);
      await unauthorized(); await pair('admin_spend_after_denial', expected);
    }
    ctx.assert(modeIntent, 'admin_spend_modes_not_exercised');
  });
  await ctx.step('step_2', async () => {
    await select('dependency_unavailable');
    for (const am of ['a', 'b']) verifySpendRefusal(await get(P + '/spend', am, 'admin_spend_unavailable',
      {allowed: [503]}), 503, 'platform_spend_unavailable');
    await unauthorized(); ctx.assert(modeIntent, 'admin_spend_dependency_not_exercised');
  });
  await ctx.step('expected_result', async () => {
    await select('enabled_without_cloud');
    await pair('admin_spend_no_observation', f.expectedWithoutCloud);
    await unauthorized(); await pair('admin_spend_after_denial', f.expectedWithoutCloud);
    await finalize(); ctx.assert(finalized, 'admin_spend_not_finalized');
  });
});
