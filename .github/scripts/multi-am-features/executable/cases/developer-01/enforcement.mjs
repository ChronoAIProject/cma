import {define, setup, settingsSession, browser, evidence, navigateSettings, saveSettings,
  required, request, read, roleRequest, P, other, id, equal, sameSettings} from './common.mjs';

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
  const custody = await f.custody(intent);
  ctx.defer('admin_created_resource_cleanup', () => custody.cleanup());
  return request(ctx, intent.method, intent.path, intent.body, {key: intent.key,
    actor: intent.actor ?? 'user', ...options});
}

export const perUserLimit = define(45, 'global', 2, async ctx => {
  const f = await setup(ctx, 'user_limit');
  required(f.emptyOwner === true && f.intents?.length === 3, 'admin_limit_empty_owner_missing');
  const state = await settingsSession(ctx, f); let changed;
  await ctx.step('step_1', async () => {
    changed = await state.patch('limits', {...state.original.settings.limits, max_sandboxes_per_user: 2});
    const responses = await Promise.all(f.intents.map((intent, i) => {
      required(intent.path === '/api/v1/sandboxes', 'admin_limit_sandbox_intent_required');
      return ownedIntent(ctx, f, intent, {am: i % 2 ? other(ctx.am) : ctx.am, allowed: [200, 201, 202, 429]});
    }));
    ctx.assert(responses.filter(r => [200, 201, 202].includes(r.status)).length === 2,
      'admin_shared_quota_admitted_wrong_count');
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
      ctx.assert(inventory.items.length === 2 && new Set(inventory.items.map(s => s.id)).size === 2,
        'admin_limit_durable_resource_count');
    }
  });
});

export const visitorLimits = define(46, 'global', 2, async ctx => {
  const f = await setup(ctx, 'visitor_limits');
  required(f.emptyVisitor === true && f.sandboxIntents?.length === 2 && f.launchIntents?.length === 3,
    'admin_visitor_boundary_fixture_missing');
  const state = await settingsSession(ctx, f); let changed;
  await ctx.step('step_1', async () => {
    changed = await state.patch('limits', {...state.original.settings.limits,
      visitor_max_sandboxes: 1, visitor_max_launches_per_day: 2});
    for (const [kind, intents, limit, code] of [
      ['sandbox', f.sandboxIntents, 1, 'sandbox_limit_reached'],
      ['launch', f.launchIntents, 2, 'resource_quota'],
    ]) {
      const responses = [];
      for (const [i, intent] of intents.entries()) {
        const custody = await f.custody(intent);
        ctx.defer('admin_visitor_resource_cleanup', () => custody.cleanup());
        responses.push(await roleRequest(ctx, f, 'visitor', 'POST', intent.path, intent.body,
          {am: i % 2 ? other(ctx.am) : ctx.am, key: intent.key, allowed: [200, 201, 202, 429]}));
      }
      ctx.assert(responses.filter(r => r.status !== 429).length === limit, 'admin_visitor_limit_count');
      ctx.assert(responses.at(-1).status === 429 && responses.at(-1).body.code === code,
        'admin_visitor_limit_refusal');
      await ctx.observe('admin_visitor_' + kind + '_outcomes', async () => responses.map(r => ({status: r.status, body: r.body})));
    }
  });
  await ctx.step('step_2', async () => {
    ctx.assert(equal((await read(ctx, P + '/settings', other(ctx.am))).settings.limits,
      changed.settings.limits), 'admin_visitor_policy_peer');
    await state.restore();
    ctx.assert(sameSettings((await read(ctx, P + '/settings')).settings, state.original.settings),
      'admin_visitor_policy_restored');
  });
  await ctx.step('expected_result', async () => {
    await roleRequest(ctx, f, 'visitor', 'GET', P + '/settings', undefined, {allowed: [403]});
    const ledger = await ctx.observe('admin_visitor_durable_counts', () => f.readPublicInventory(ctx));
    ctx.assert(ledger.sandbox_ids.length === 1 && ledger.launch_ids.length === 2
      && new Set(ledger.launch_ids).size === 2, 'admin_visitor_duplicate_or_privileged_effect');
  });
});

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

export const idlePause = define(49, 'browser', 3, async ctx => {
  const f = await setup(ctx, 'idle_pause');
  required(f.exclusiveSettings && f.cohort?.some(s => s.kind === 'idle')
    && f.cohort.some(s => s.kind === 'busy') && f.cohort.some(s => s.kind === 'always_on'),
  'admin_pause_cohort_missing');
  const s = await browser(ctx); let acknowledged;
  await ctx.step('step_1', async () => {
    for (const sandbox of f.cohort) {
      const state = (await read(ctx, P + '/sandboxes/' + id(sandbox.id))).sandbox;
      ctx.assert(state.status === 'active', 'admin_pause_initial_state');
      if (sandbox.kind === 'always_on') ctx.assert(state.profile.always_on, 'admin_pause_always_on_fixture');
    }
  });
  await ctx.step('step_2', async () => {
    await request(ctx, 'POST', P + '/auto-pause/pause-idle-now', {confirmed: false}, {allowed: [400]});
    await navigateSettings(s);
    const response = s.page.waitForResponse(r => new URL(r.url()).pathname === P + '/auto-pause/pause-idle-now'
      && r.request().method() === 'POST');
    s.page.once('dialog', dialog => dialog.accept());
    await s.page.getByRole('button', {name: 'Pause idle now', exact: true}).click();
    acknowledged = await (await response).json();
    ctx.assert(['scheduled', 'already_satisfied', 'skipped'].every(k => Number.isSafeInteger(acknowledged[k])
      && acknowledged[k] >= 0), 'admin_pause_ack_shape');
    ctx.assert(acknowledged.scheduled === f.cohort.filter(s => s.kind === 'idle').length,
      'admin_pause_scheduled_wrong_cohort');
    await ctx.observe('admin_pause_ack', async () => acknowledged);
  });
  await ctx.step('step_3', async () => {
    await poll(async () => {
      const states = [];
      for (const am of ['a', 'b']) for (const sandbox of f.cohort) {
        const state = (await read(ctx, P + '/sandboxes/' + id(sandbox.id), am)).sandbox;
        if (sandbox.kind !== 'idle') ctx.assert(state.status === 'active', 'admin_pause_protected_work_changed');
        states.push({kind: sandbox.kind, status: state.status});
      }
      return states;
    }, states => states.every(s => s.status === (s.kind === 'idle' ? 'paused' : 'active')),
    'admin_pause_lifecycle_timeout');
    ctx.assert(true, 'admin_pause_lifecycle_observed');
  });
  await ctx.step('expected_result', async () => {
    await evidence(ctx, s);
    const work = await ctx.observe('admin_pause_busy_progress', () => f.readBusyWork(ctx));
    ctx.assert(work.sequence > f.initialSequence && work.restarts === 0, 'admin_pause_busy_turn_interrupted');
  });
}, ['admin-fixtures', 'browser']);
