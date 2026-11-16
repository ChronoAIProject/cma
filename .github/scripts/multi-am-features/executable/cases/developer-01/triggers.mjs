import {define, setup, settingsSession, request, read, required, P, other, equal,
  browser, evidence, navigateSettings, saveSettings, sameSettings} from './common.mjs';
import {poll} from './enforcement.mjs';

async function launchSnapshot(ctx, f) {
  const result = await ctx.observe('admin_trigger_launches', () => f.readLaunches(ctx));
  required(Array.isArray(result.launches) && result.trigger_id === f.triggerId,
    'admin_trigger_launch_snapshot_missing');
  return result.launches;
}

export const triggerSetting = define(47, 'browser', 2, async ctx => {
  const f = await setup(ctx, 'trigger_settings'), state = await settingsSession(ctx, f);
  required(typeof f.readLaunches === 'function' && typeof f.waitDispatchBoundary === 'function'
    && f.pendingOccurrenceId, 'admin_pending_trigger_fixture_missing');
  const s = await browser(ctx); let prior;
  await ctx.step('step_1', async () => {
    prior = await launchSnapshot(ctx, f);
    for (const [enabled, paused] of [[false, false], [true, true], [true, false]]) {
      await navigateSettings(s);
      const before = await read(ctx, P + '/settings');
      const desired = {...before.settings, triggers: {...before.settings.triggers,
        enabled, orchestration_paused: paused}}; state.own(desired);
      await s.page.getByLabel('Triggers enabled', {exact: true}).setChecked(enabled);
      await s.page.getByLabel('Orchestration paused', {exact: true}).setChecked(paused);
      await saveSettings(s);
      const peer = await read(ctx, P + '/settings', other(ctx.am));
      ctx.assert(equal(peer.settings.triggers, desired.triggers), 'admin_trigger_settings_peer');
      await f.waitDispatchBoundary();
      const launches = await launchSnapshot(ctx, f);
      if (!enabled || paused) ctx.assert(equal(launches, prior), 'admin_paused_trigger_launched');
      else await poll(() => launchSnapshot(ctx, f), rows => rows.some(r => r.occurrence_id === f.pendingOccurrenceId),
        'admin_trigger_did_not_resume');
    }
  });
  await ctx.step('step_2', async () => {
    const launches = await launchSnapshot(ctx, f);
    ctx.assert(launches.filter(r => r.occurrence_id === f.pendingOccurrenceId).length === 1,
      'admin_trigger_duplicate_launch');
    await state.restore();
    ctx.assert(sameSettings((await read(ctx, P + '/settings')).settings, state.original.settings),
      'admin_trigger_settings_restore');
  });
  await ctx.step('expected_result', async () => { await evidence(ctx, s); });
}, ['admin-fixtures', 'browser']);

function triggerCommand(number, action) {
  return define(number, 'global', 2, async ctx => {
    const f = await setup(ctx, 'trigger_commands'), state = await settingsSession(ctx, f);
    required(typeof f.readLaunches === 'function' && typeof f.waitDispatchBoundary === 'function'
      && f.pendingOccurrenceId, 'admin_trigger_command_fixture_missing');
    let prior, revision;
    await ctx.step('step_1', async () => {
      prior = await launchSnapshot(ctx, f);
      const before = await read(ctx, P + '/settings');
      const initial = {...before.settings.triggers, enabled: true, orchestration_paused: action === 'resume'};
      if (!equal(before.settings.triggers, initial)) await state.patch('triggers', initial);
      const ready = await read(ctx, P + '/settings');
      state.own({...ready.settings, triggers: {...ready.settings.triggers, orchestration_paused: action === 'pause'}});
      await request(ctx, 'POST', P + '/triggers/' + action);
      const current = await read(ctx, P + '/settings', other(ctx.am)); revision = current.revision;
      ctx.assert(current.settings.triggers.orchestration_paused === (action === 'pause')
        && revision > ready.revision, 'admin_trigger_command_not_applied');
    });
    await ctx.step('step_2', async () => {
      await f.waitDispatchBoundary();
      if (action === 'resume') await poll(() => launchSnapshot(ctx, f),
        rows => rows.some(r => r.occurrence_id === f.pendingOccurrenceId), 'admin_trigger_resume_timeout');
      const rows = await launchSnapshot(ctx, f);
      ctx.assert(action === 'pause' ? equal(rows, prior)
        : rows.filter(r => r.occurrence_id === f.pendingOccurrenceId).length === 1,
      'admin_trigger_command_duplicate_or_missing');
      for (const am of ['a', 'b']) {
        const current = await read(ctx, P + '/settings', am);
        ctx.assert(current.settings.triggers.orchestration_paused === (action === 'pause'),
          'admin_trigger_command_peer');
      }
      await state.restore();
      ctx.assert(sameSettings((await read(ctx, P + '/settings')).settings, state.original.settings),
        'admin_trigger_command_restore');
    });
    await ctx.step('expected_result', async () => {
      const audit = await read(ctx, P + '/activity?action=configure&limit=50');
      ctx.assert(audit.items.filter(r => r.target?.id === 'triggers' && r.detail?.revision === String(revision)
        && r.outcome?.status === 'succeeded').length === 1, 'admin_trigger_command_audit');
    });
  });
}
export const triggerPause = triggerCommand(75, 'pause');
export const triggerResume = triggerCommand(76, 'resume');
