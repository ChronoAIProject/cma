import {define, setup, settingsSession, invalidSettings, required, request, read, P, other, equal, sameSettings} from './common.mjs';
import {ownedIntent} from './enforcement.mjs';
import {automaticPauseScenario} from './automatic-pause.mjs';

export const automaticPause = define(42, 'browser', 2, ctx => automaticPauseScenario(ctx), ['admin-fixtures', 'browser']);

export const defaultsMarketplace = define(50, 'global', 2, async ctx => {
  const f = await setup(ctx, 'defaults_marketplace'), state = await settingsSession(ctx, f);
  required(f.defaults && f.invalidDefaults?.length && f.creation?.path
    && f.catalogPath && f.expectedCatalogIds?.length, 'admin_default_profile_fixture_missing');
  required(f.creation.path === '/api/v1/conversations' && f.creation.method === 'POST'
    && f.creation.actor === 'user' && equal(f.creation.body, {}),
    'admin_default_driven_creation_required');
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
    const created = await ownedIntent(ctx, f, f.creation, {am: other(ctx.am), allowed: [200]});
    ctx.assert(created.body.object === 'conversation' && typeof created.body.id === 'string'
      && ['a', 'b'].includes(created.timing.am), 'admin_default_creation_receipt');
    required(typeof f.creationStatePath === 'function', 'admin_default_creation_readback_missing');
    const hierarchy = (await request(ctx, 'GET', f.creationStatePath(created.body), undefined,
      {actor: 'user', am: other(created.timing.am), agentId: created.body.id})).body;
    for (const kind of ['agent', 'workspace', 'sandbox']) {
      const profile = kind === 'agent' ? hierarchy.profile : hierarchy[kind]?.profile;
      ctx.assert(profile?.id === f.defaults[kind + '_profile'].id
        && profile.revision === f.defaults[kind + '_profile'].revision, 'admin_default_not_used_by_creation');
    }
    const catalog = await request(ctx, 'GET', f.catalogPath, undefined, {actor: 'user', allowed: [200, 503]});
    ctx.assert(changed.settings.marketplace.enabled
      ? catalog.status === 200 && f.expectedCatalogIds.every(value => catalog.body.items.some(p => p.id === value))
      : catalog.status === 503 && catalog.body.code === 'marketplace_disabled', 'admin_marketplace_policy_not_enforced');
    for (const defaults of f.invalidDefaults) {
      // Retain the negative-test intent too: an erroneous server acceptance
      // must fail the case without stranding the shared settings on that value.
      state.own({...changed.settings, defaults});
      await invalidSettings(ctx, {defaults}, [400, 404, 409, 422]);
    }
    await state.restore();
    ctx.assert(sameSettings((await read(ctx, P + '/settings')).settings, state.original.settings),
      'admin_defaults_restore');
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(equal((await read(ctx, P + '/settings', other(ctx.am))).settings.defaults,
      state.original.settings.defaults), 'admin_invalid_defaults_partial_overwrite');
  });
});
