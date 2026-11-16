import {define, setup, settingsSession, browser, evidence, navigateSettings, saveSettings,
  required, request, read, roleRequest, P, other, equal, sameSettings} from './common.mjs';
import {visitorSettingsUI} from './visitor-settings-ui.mjs';
import {pauseIdleScenario} from './idle-pause.mjs';

export async function poll(operation, complete, code, timeout = 120000) {
  const deadline = performance.now() + timeout;
  for (;;) {
    const value = await operation();
    if (complete(value)) return value;
    if (performance.now() >= deadline) throw Error(code);
    await new Promise(resolve => setTimeout(resolve, 1000));
  }
}

export async function ownedIntent(ctx, f, intent, options = {}) {
  required(intent?.key && intent.method === 'POST' && typeof f.custody === 'function',
    'admin_owned_intent_missing');
  const {requireCapture = false, ...requestOptions} = options;
  const dispatch = {key: intent.key, actor: intent.actor ?? 'user', am: ctx.am, ...requestOptions};
  const custody = await f.custody(intent, {actor: dispatch.actor, am: dispatch.am});
  required(typeof custody?.cleanup === 'function', 'admin_owned_cleanup_missing');
  ctx.defer('admin_created_resource_cleanup', () => custody.cleanup());
  required(!requireCapture || typeof custody.capture === 'function', 'admin_owned_capture_missing');
  let response;
  try {
    response = await request(ctx, intent.method, intent.path, intent.body, dispatch);
  } catch (error) {
    // Attribution may reject an already accepted create. Retain its original
    // response for cleanup without retrying the mutation or losing the failure.
    if (error.response && custody.capture) {
      try {await custody.capture(error.response);} catch (captureError) {
        throw new AggregateError([error, captureError], 'admin_owned_capture_failed');
      }
    }
    throw error;
  }
  await custody.capture?.(response);
  return response;
}

export const perUserLimit = define(45, 'global', 2, async ctx => {
  const f = await setup(ctx, 'user_limit');
  required(f.emptyOwner === true && f.intents?.length === 3, 'admin_limit_empty_owner_missing');
  const state = await settingsSession(ctx, f); let changed, admittedIds;
  await ctx.step('step_1', async () => {
    changed = await state.patch('limits', {...state.original.settings.limits, max_sandboxes_per_user: 2});
    const outcomes = await Promise.allSettled(f.intents.map(async (intent, i) => {
      required(intent.path === '/api/v1/sandboxes', 'admin_limit_sandbox_intent_required');
      return ownedIntent(ctx, f, intent, {am: i % 2 ? other(ctx.am) : ctx.am, allowed: [201, 429]});
    }));
    // Drain every attempted create before returning control to case cleanup.
    const failures = outcomes.filter(r => r.status === 'rejected').map(r => r.reason);
    if (failures.length) throw new AggregateError(failures, 'admin_limit_create_failed');
    const responses = outcomes.map(r => r.value);
    const admitted = responses.filter(r => r.status === 201);
    ctx.assert(admitted.length === 2,
      'admin_shared_quota_admitted_wrong_count');
    admittedIds = admitted.map(r => r.body?.id);
    ctx.assert(admittedIds.every(value => typeof value === 'string' && value.trim())
      && new Set(admittedIds).size === 2, 'admin_shared_quota_resource_identity');
    const refused = responses.filter(r => r.status === 429);
    ctx.assert(refused.length === 1 && refused[0].body.code === 'sandbox_limit_reached',
      'admin_shared_quota_missing_refusal');
  });
  await ctx.step('step_2', async () => {
    const peer = await read(ctx, P + '/settings', other(ctx.am));
    ctx.assert(equal(peer.settings.limits, changed.settings.limits), 'admin_limit_policy_peer');
    await state.restore();
    ctx.assert(sameSettings((await read(ctx, P + '/settings')).settings, state.original.settings),
      'admin_limit_policy_restored');
  });
  await ctx.step('expected_result', async () => {
    for (const am of ['a', 'b']) {
      const inventory = (await request(ctx, 'GET', '/api/v1/sandboxes?limit=100', undefined,
        {actor: 'user', am})).body;
      ctx.assert(inventory.next == null && inventory.items.length === 2
        && equal(inventory.items.map(s => s.id).sort(), admittedIds.toSorted()),
        'admin_limit_durable_resource_count');
    }
  });
});

export const visitorLimits = define(46, 'browser', 2, async ctx => {
  const f = await setup(ctx, 'visitor_limits');
  required(f.emptyVisitor === true && Array.isArray(f.sandboxIntents) && f.sandboxIntents.length === 2
    && Array.isArray(f.launchIntents) && f.launchIntents.length === 3
    && typeof f.custody === 'function' && typeof f.readPublicInventory === 'function',
  'admin_visitor_boundary_fixture_missing');
  const all = [...f.sandboxIntents, ...f.launchIntents];
  required(all.every(intent => intent?.method === 'POST' && (intent.actor ?? 'visitor') === 'visitor'
    && typeof intent.key === 'string' && intent.key.trim() && intent.body && typeof intent.body === 'object'
    && !Array.isArray(intent.body)) && new Set(all.map(intent => intent.key)).size === all.length
    && f.sandboxIntents.every(intent => intent.path === '/api/v1/sandboxes')
    && f.launchIntents.every(intent => /^\/api\/v1\/triggers\/public\/[-a-zA-Z0-9_]{1,160}\/launches$/.test(intent.path)),
  'admin_visitor_intent_fixture_invalid');
  const state = await settingsSession(ctx, f), acceptedIds = {}; let changed;
  const ui = await visitorSettingsUI(ctx, state);
  await ctx.step('step_1', async () => {
    changed = await ui.change();
    for (const [kind, intents, limit, code] of [
      ['sandbox', f.sandboxIntents, 1, 'sandbox_limit_reached'],
      ['launch', f.launchIntents, 2, 'resource_quota'],
    ]) {
      const responses = [];
      for (const [i, intent] of intents.entries()) {
        // Retain original accepted/refused responses even if request attribution
        // fails later. Visitor identity is explicit; never borrow the admin.
        responses.push(await ownedIntent(ctx, f, intent, {actor: 'visitor',
          am: i % 2 ? other(ctx.am) : ctx.am, requireCapture: true,
          allowed: kind === 'sandbox' ? [201, 429] : [201, 202, 429]}));
      }
      await ctx.observe('admin_visitor_' + kind + '_outcomes', async () => responses);
      ctx.assert(responses.every((r, i) => r.timing?.am === (i % 2 ? other(ctx.am) : ctx.am)
        && r.timing.request_id?.trim()), 'admin_visitor_ingress');
      const accepted = responses.filter(r => r.status !== 429);
      ctx.assert(accepted.length === limit, 'admin_visitor_limit_count');
      acceptedIds[kind] = accepted.map(r => r.body?.id);
      ctx.assert(acceptedIds[kind].every(value => typeof value === 'string' && value.trim())
        && new Set(acceptedIds[kind]).size === limit, 'admin_visitor_resource_identity');
      const refusal = responses.at(-1);
      ctx.assert(refusal.status === 429 && refusal.body.code === code, 'admin_visitor_limit_refusal');
      if (kind === 'launch') ctx.assert(refusal.body.resource === 'visitor_launches'
        && refusal.body.limit === limit && Number.isFinite(Date.parse(refusal.body.reset_at)),
      'admin_visitor_launch_quota_scope');
    }
  });
  await ctx.step('step_2', async () => {
    ctx.assert(equal((await read(ctx, P + '/settings', other(ctx.am))).settings.limits,
      changed.settings.limits), 'admin_visitor_policy_peer');
    await ui.close();
    await state.restore();
    ctx.assert(sameSettings((await read(ctx, P + '/settings')).settings, state.original.settings),
      'admin_visitor_policy_restored');
  });
  await ctx.step('expected_result', async () => {
    await roleRequest(ctx, f, 'visitor', 'GET', P + '/settings', undefined, {allowed: [403]});
    const ledger = await ctx.observe('admin_visitor_durable_counts', () => f.readPublicInventory(ctx));
    ctx.assert(Array.isArray(ledger?.sandbox_ids) && Array.isArray(ledger.launch_ids)
      && equal(ledger.sandbox_ids.toSorted(), acceptedIds.sandbox.toSorted())
      && equal(ledger.launch_ids.toSorted(), acceptedIds.launch.toSorted()), 'admin_visitor_durable_identity');
  });
}, ['admin-fixtures', 'browser']);

export const measuredVisibility = define(43, 'browser', 2, async ctx => {
  const f = await setup(ctx, 'measured_visibility'), state = await settingsSession(ctx, f);
  required(f.measurementGapMarker && f.completedUsage === true, 'admin_measured_report_fixture_missing');
  const settings = await browser(ctx); let updated;
  await ctx.step('step_1', async () => {
    for (const enabled of [!state.original.settings.usage_reporting.measured_visible,
      state.original.settings.usage_reporting.measured_visible]) {
      await navigateSettings(settings);
      const desired = structuredClone((await read(ctx, P + '/settings')).settings);
      desired.usage_reporting.measured_visible = enabled; state.own(desired);
      await settings.page.getByLabel('Show measured usage', {exact: true}).setChecked(enabled);
      await saveSettings(settings); updated = await read(ctx, P + '/settings');
      for (const actor of ['user', 'admin']) {
        const s = await browser(ctx, actor, other(ctx.am));
        await s.page.goto(new URL(actor === 'admin' ? '/operations' : '/', s.origin).href);
        if (actor === 'user') {
          await s.page.getByRole('button', {name: /^Account menu for /}).click();
          await s.page.getByRole('menuitem', {name: 'Usage', exact: true}).click();
          await s.page.getByRole('dialog', {name: 'Your usage', exact: true}).waitFor();
        }
        await s.page.getByRole('region', {name: 'Allocated capacity now', exact: true}).waitFor();
        ctx.assert(await s.page.getByRole('region', {name: 'Allocated resource-hours', exact: true}).isVisible(),
          'admin_measured_toggle_hid_allocation');
        const measured = s.page.getByRole('region', {name: 'Measured resource charts', exact: true});
        if (enabled) await measured.waitFor();
        ctx.assert(await measured.isVisible() === enabled, 'admin_measured_visibility_mismatch');
        if (enabled) ctx.assert(await measured.getByText(f.measurementGapMarker, {exact: true}).isVisible(),
          'admin_missing_measurement_fabricated_zero');
        await evidence(ctx, s);
      }
    }
  });
  await ctx.step('step_2', async () => {
    ctx.assert(equal((await read(ctx, P + '/settings', other(ctx.am))).settings.usage_reporting,
      updated.settings.usage_reporting), 'admin_measured_policy_peer');
    await state.restore();
    ctx.assert(sameSettings((await read(ctx, P + '/settings')).settings, state.original.settings),
      'admin_measured_restore');
  });
  await ctx.step('expected_result', async () => { await evidence(ctx, settings); });
}, ['admin-fixtures', 'browser']);

export const idlePause = define(49, 'browser', 3, ctx => pauseIdleScenario(ctx, poll), ['admin-fixtures', 'browser']);
