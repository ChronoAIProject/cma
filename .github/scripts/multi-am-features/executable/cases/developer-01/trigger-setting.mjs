import {setup, settingsSession, read, P, other, sameSettings} from './common.mjs';
import {triggerSettingsUI} from './trigger-settings-ui.mjs';
import {launchFixture, launchIdentity, launchInventory, launchPair, allPending, isPaused,
  isReady, readyResources, publishedTrigger} from './trigger-launch-proof.mjs';

export async function triggerSettingScenario(ctx, {poll, ownedIntent}) {
  const f = await setup(ctx, 'trigger_settings'); launchFixture(f);
  const state = await settingsSession(ctx, f), ui = await triggerSettingsUI(ctx, state);
  const create = async (intent, am, allowed) => {
    try {return await ownedIntent(ctx, f, intent, {actor: 'user', am, allowed, requireCapture: true});}
    finally {
      // Resource custody registers during dispatch, including unknown outcomes.
      // Close/drain the browser before those newer resource cleanup hooks run.
      ctx.defer('admin_trigger_close_before_resources', ui.close);
    }
  };
  let admitted;
  await ctx.step('step_1', async () => {
    await publishedTrigger(ctx, f);
    for (const am of ['a', 'b']) await launchInventory(ctx, f, am);
    await ui.change(false, false);
    for (const [index, am] of ['a', 'b'].entries()) {
      const refused = await create(f.disabledIntents[index], am, [503]);
      ctx.assert(refused.status === 503 && refused.body?.code === 'triggers_disabled', 'admin_trigger_disabled_refusal');
      await launchInventory(ctx, f, other(am));
    }
    await ui.change(true, true);
    const response = await create(f.launchIntent, ctx.am, [201, 202]);
    admitted = structuredClone(response.body); launchIdentity(ctx, f, admitted);
    ctx.assert(admitted.state === 'resolving' && allPending(admitted), 'admin_trigger_initial_launch');
    const paused = await poll(async () => {
      const pair = await launchPair(ctx, f, admitted);
      ctx.assert(pair.every(({launch}) => allPending(launch)), 'admin_trigger_paused_resources_changed');
      ctx.assert(pair.every(({launch}) => launch.blocked_reason == null || isPaused(launch)), 'admin_trigger_wrong_block');
      return pair;
    }, pair => pair.every(({launch}) => isPaused(launch)), 'admin_trigger_pause_timeout');
    await ctx.observe('admin_trigger_paused_launch', async () => paused);
    ctx.assert(paused.every(({launch}) => isPaused(launch) && allPending(launch)), 'admin_trigger_pause_unproven');
    for (const am of ['a', 'b']) await launchInventory(ctx, f, am, [admitted.id]);
    await ui.change(true, false);
  });
  await ctx.step('step_2', async () => {
    await ui.close();
    const completed = await poll(() => launchPair(ctx, f, admitted),
      pair => pair.every(({launch}) => isReady(launch)), 'admin_trigger_resume_timeout');
    readyResources(ctx, completed);
    await ctx.observe('admin_trigger_completed_launch', async () => completed);
    for (const am of ['a', 'b']) await launchInventory(ctx, f, am, [admitted.id]);
    await state.restore();
    ctx.assert(sameSettings((await read(ctx, P + '/settings', other(ctx.am))).settings, state.original.settings),
      'admin_trigger_settings_restore');
  });
  await ctx.step('expected_result', async () => {
    // Restoration is complete and the browser is drained. Keep exact identity
    // and resource checks through final readback; never create another Launch.
    const final = await launchPair(ctx, f, admitted); readyResources(ctx, final);
    for (const am of ['a', 'b']) await launchInventory(ctx, f, am, [admitted.id]);
  });
}
