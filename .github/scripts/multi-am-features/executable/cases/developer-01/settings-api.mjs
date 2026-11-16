import {define, setup, settingsSession, invalidSettings, request, read, roleRequest,
  P, other, equal, sameSettings, required} from './common.mjs';

export const settingsPatch = define(51, 'global', 3, async ctx => {
  const f = await setup(ctx, 'settings_patch');
  required(f.resetDefaults && Array.isArray(f.patchSections) && f.patchSections.length,
    'admin_source_bound_defaults_missing');
  required(['model_budget', 'usage_reporting', 'defaults', 'inactivity', 'retention', 'limits',
    'triggers', 'marketplace'].every(section => f.patchSections.some(item => item.section === section)),
  'admin_supported_patch_sections_missing');
  const state = await settingsSession(ctx, f);
  let before, updated;
  await ctx.step('step_1', async () => {
    for (const {section, value} of f.patchSections) {
      before = await read(ctx, P + '/settings');
      required(section in before.settings, 'admin_patch_section_unknown');
      updated = await state.patch(section, value);
      const seen = await read(ctx, P + '/settings', other(ctx.am));
      ctx.assert(seen.revision === updated.revision && equal(seen.settings[section], value),
        'admin_patch_peer_value');
      for (const key of Object.keys(before.settings).filter(key => key !== section)) {
        ctx.assert(equal(seen.settings[key], before.settings[key]), 'admin_patch_omitted_section_changed');
      }
    }
  });
  await ctx.step('step_2', async () => {
    await invalidSettings(ctx, {unknown_section: {enabled: true}});
    await invalidSettings(ctx, {});
    const latest = await read(ctx, P + '/settings');
    await request(ctx, 'PATCH', P + '/settings', {
      expected_revision: before.revision, retention: before.settings.retention,
    }, {allowed: [409], am: other(ctx.am)});
    ctx.assert(equal(latest, await read(ctx, P + '/settings')), 'admin_stale_patch_changed_state');
  });
  await ctx.step('step_3', async () => {
    const current = await read(ctx, P + '/settings');
    state.own(f.resetDefaults);
    const reset = (await request(ctx, 'POST', P + '/settings/reset', {expected_revision: current.revision})).body;
    ctx.assert(reset.revision > current.revision && sameSettings(reset.settings, f.resetDefaults),
      'admin_reset_defaults_mismatch');
    const peer = await read(ctx, P + '/settings', other(ctx.am));
    ctx.assert(sameSettings(peer.settings, reset.settings), 'admin_reset_peer_mismatch');
    const audit = await read(ctx, P + '/activity?action=configure&limit=50');
    ctx.assert(audit.items.some(r => r.target?.id === 'platform_settings'
      && r.detail?.revision === String(reset.revision) && r.outcome?.status === 'succeeded'),
    'admin_reset_audit_missing');
  });
  await ctx.step('expected_result', async () => {
    await roleRequest(ctx, f, 'user', 'PATCH', P + '/settings', {
      expected_revision: updated.revision, retention: updated.settings.retention,
    }, {allowed: [403]});
    await state.restore();
    ctx.assert(sameSettings((await read(ctx, P + '/settings')).settings, state.original.settings),
      'admin_patch_reset_restore');
  });
});
