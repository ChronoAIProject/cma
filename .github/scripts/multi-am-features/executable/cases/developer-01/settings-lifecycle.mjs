import {define, setup, settingsSession, browser, evidence, navigateSettings, saveSettings,
  invalidSettings, required, request, read, P, other, equal, sameSettings, id} from './common.mjs';
import {poll, ownedIntent} from './enforcement.mjs';

export const automaticPause = define(42, 'browser', 2, async ctx => {
  const f = await setup(ctx, 'automatic_pause'), state = await settingsSession(ctx, f);
  required(f.cohort?.some(s => s.kind === 'idle') && f.cohort.some(s => s.kind === 'busy')
    && f.cohort.some(s => s.kind === 'always_on') && typeof f.waitIdleThreshold === 'function',
  'admin_automatic_pause_fixture_missing');
  const s = await browser(ctx);
  await ctx.step('step_1', async () => {
    await navigateSettings(s);
    const desired = structuredClone(state.original.settings);
    desired.inactivity = {...desired.inactivity, auto_pause_enabled: false, pause_after_seconds: 60};
    state.own(desired);
    await s.page.getByLabel('Enable auto pause', {exact: true}).setChecked(false);
    const seconds = s.page.getByLabel('Pause after (seconds)', {exact: true});
    for (const invalid of ['59', '86401']) {
      await seconds.fill(invalid);
      ctx.assert(!(await seconds.evaluate(node => node.checkValidity())), 'admin_idle_threshold_ui_bounds');
      await invalidSettings(ctx, {inactivity: {...desired.inactivity, pause_after_seconds: Number(invalid)}});
    }
    // Register the boundary intent before dispatch for unknown acknowledgements.
    state.own({...desired, inactivity: {...desired.inactivity, pause_after_seconds: 86400}});
    await seconds.fill('86400'); await saveSettings(s);
    await seconds.fill('60'); await saveSettings(s);
    state.own({...desired, inactivity: {...desired.inactivity, auto_pause_enabled: true}});
    await s.page.getByLabel('Enable auto pause', {exact: true}).check();
    await s.page.getByLabel('Pause after hours', {exact: true}).fill('0');
    await s.page.getByLabel('Pause after minutes', {exact: true}).fill('1');
    await s.page.getByLabel('Pause after seconds', {exact: true}).fill('0');
    await saveSettings(s);
    const enabled = await read(ctx, P + '/settings', other(ctx.am));
    ctx.assert(enabled.settings.inactivity.auto_pause_enabled === true
      && enabled.settings.inactivity.pause_after_seconds === 60, 'admin_idle_threshold_conversion');
    const time = await ctx.observe('admin_actual_idle_threshold', () => f.waitIdleThreshold(60));
    ctx.assert(time.elapsed_seconds >= 60 && time.grace_elapsed === true, 'admin_idle_threshold_not_elapsed');
    await poll(async () => {
      const rows = [];
      for (const am of ['a', 'b']) for (const fixture of f.cohort) {
        const sandbox = (await read(ctx, P + '/sandboxes/' + id(fixture.id), am)).sandbox;
        if (fixture.kind !== 'idle') ctx.assert(sandbox.status === 'active', 'admin_auto_pause_protected');
        rows.push({kind: fixture.kind, status: sandbox.status});
      }
      return rows;
    }, rows => rows.every(r => r.status === (r.kind === 'idle' ? 'paused' : 'active')),
    'admin_auto_pause_timeout');
  });
  await ctx.step('step_2', async () => {
    await state.restore();
    ctx.assert(sameSettings((await read(ctx, P + '/settings', other(ctx.am))).settings, state.original.settings),
      'admin_auto_pause_restore');
  });
  await ctx.step('expected_result', async () => { await evidence(ctx, s); });
}, ['admin-fixtures', 'browser']);

export const defaultsMarketplace = define(50, 'global', 2, async ctx => {
  const f = await setup(ctx, 'defaults_marketplace'), state = await settingsSession(ctx, f);
  required(f.defaults && f.invalidDefaults?.length && f.creation?.path
    && f.catalogPath && f.expectedCatalogIds?.length, 'admin_default_profile_fixture_missing');
  let changed;
  await ctx.step('step_1', async () => {
    for (const kind of ['agent', 'workspace', 'sandbox']) {
      const ref = f.defaults[kind + '_profile'];
      required(ref?.id && Number.isSafeInteger(ref.revision) && ref.revision > 0,
        'admin_default_revision_required');
    }
    changed = await state.patch('defaults', f.defaults);
    changed = await state.patch('marketplace', {enabled: !state.original.settings.marketplace.enabled});
    ctx.assert(equal(changed.settings.defaults, f.defaults), 'admin_default_save');
  });
  await ctx.step('step_2', async () => {
    const peer = await read(ctx, P + '/settings', other(ctx.am));
    ctx.assert(sameSettings(peer.settings, changed.settings), 'admin_default_peer');
    const created = await ownedIntent(ctx, f, f.creation, {am: other(ctx.am), allowed: [200, 201, 202]});
    required(typeof f.creationStatePath === 'function', 'admin_default_creation_readback_missing');
    const hierarchy = (await request(ctx, 'GET', f.creationStatePath(created.body), undefined,
      {actor: 'user', am: ctx.am})).body;
    for (const kind of ['agent', 'workspace', 'sandbox']) {
      const profile = kind === 'agent' ? hierarchy.profile : hierarchy[kind]?.profile;
      ctx.assert(profile?.id === f.defaults[kind + '_profile'].id
        && profile.revision === f.defaults[kind + '_profile'].revision, 'admin_default_not_used_by_creation');
    }
    const catalog = await request(ctx, 'GET', f.catalogPath, undefined, {actor: 'user', allowed: [200, 503]});
    ctx.assert(changed.settings.marketplace.enabled
      ? catalog.status === 200 && f.expectedCatalogIds.every(value => catalog.body.items.some(p => p.id === value))
      : catalog.status === 503 && catalog.body.code === 'marketplace_disabled', 'admin_marketplace_policy_not_enforced');
    for (const defaults of f.invalidDefaults) await invalidSettings(ctx, {defaults}, [400, 404, 409, 422]);
    await state.restore();
    ctx.assert(sameSettings((await read(ctx, P + '/settings')).settings, state.original.settings),
      'admin_defaults_restore');
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(equal((await read(ctx, P + '/settings', other(ctx.am))).settings.defaults,
      state.original.settings.defaults), 'admin_invalid_defaults_partial_overwrite');
  });
});
