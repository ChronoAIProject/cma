import {define, setup, browser, evidence, navigateSettings, roleRequest, read,
  P, other, equal, required} from './common.mjs';

export async function activityPages(ctx, params = {}, {summary = false, role, fixture} = {}) {
  const rows = [], seen = new Set(), cursors = new Set(); let cursor, pages = 0;
  do {
    const query = new URLSearchParams({...params, limit: '2', ...(summary ? {view: 'summary'} : {}),
      ...(cursor ? {cursor} : {})});
    const am = pages % 2 ? other(ctx.am) : ctx.am;
    const page = role ? (await roleRequest(ctx, fixture, role, 'GET', P + '/activity?' + query,
      undefined, {am})).body : await read(ctx, P + '/activity?' + query, am);
    ctx.assert(Array.isArray(page.items) && (page.next === null || typeof page.next === 'string'),
      'admin_activity_page_shape');
    for (const row of page.items) {
      ctx.assert(typeof row.id === 'string' && !seen.has(row.id), 'admin_activity_duplicate');
      ctx.assert(Number.isFinite(Date.parse(row.at)) && row.outcome?.status, 'admin_activity_record_shape');
      ctx.assert(summary ? !Object.hasOwn(row, 'detail') : row.detail && typeof row.detail === 'object',
        'admin_activity_projection_shape');
      if (params.actor) ctx.assert(row.actor === params.actor, 'admin_activity_actor_filter');
      if (params.action) ctx.assert(row.action === params.action, 'admin_activity_action_filter');
      seen.add(row.id); rows.push(row);
    }
    cursor = page.next;
    ctx.assert(++pages <= 100 && (!cursor || !cursors.has(cursor)), 'admin_activity_cursor_loop');
    if (cursor) cursors.add(cursor);
  } while (cursor);
  return rows;
}

export const activityFull = define(71, 'global', 2, async ctx => {
  const f = await setup(ctx, 'activity_full');
  required(Array.isArray(f.records) && f.records.length >= 3
    && ['configure', 'delete'].every(action => f.records.some(r => r.action === action))
    && typeof f.faults?.failNextRead === 'function', 'admin_activity_fixture_missing');
  let full;
  await ctx.step('step_1', async () => {
    full = await activityPages(ctx, {actor: f.actor});
    for (const expected of f.records) {
      const observed = full.find(row => row.id === expected.id);
      ctx.assert(observed && equal(observed.detail, expected.detail)
        && equal(observed.outcome, expected.outcome) && observed.action === expected.action
        && equal(observed.target, expected.target), 'admin_activity_retained_detail_mismatch');
    }
  });
  await ctx.step('step_2', async () => {
    required(Array.isArray(f.receipts) && f.receipts.length >= 2, 'admin_action_receipts_missing');
    for (const receipt of f.receipts) {
      const row = full.find(r => r.id === receipt.activity_id);
      ctx.assert(row && row.action === receipt.action && equal(row.target, receipt.target)
        && equal(row.outcome, receipt.outcome) && row.detail.revision === String(receipt.revision),
      'admin_audit_request_receipt_mismatch');
    }
    const summary = await activityPages(ctx, {actor: f.actor}, {summary: true});
    ctx.assert(equal(summary.map(row => row.id), full.map(row => row.id)), 'admin_activity_projection_cohort');
    await roleRequest(ctx, f, 'user', 'GET', P + '/activity', undefined, {allowed: [403]});
  });
  await ctx.step('expected_result', async () => {
    const fault = await f.faults.failNextRead({path: P + '/activity'});
    ctx.defer('admin_activity_fault_cleanup', () => fault.cleanup());
    const failed = await roleRequest(ctx, f, 'activity_operator', 'GET', P + '/activity',
      undefined, {allowed: [503]});
    ctx.assert(!Array.isArray(failed.body?.items), 'admin_activity_failure_fabricated_success');
    ctx.assert(f.records.some(row => row.outcome?.status !== 'succeeded')
      && full.some(row => row.outcome?.status !== 'succeeded'), 'admin_activity_failed_action_lost');
  });
});

export const settingsAudit = define(52, 'browser', 2, async ctx => {
  const f = await setup(ctx, 'settings_audit'), session = await browser(ctx);
  required(typeof f.useAuditCohort === 'function', 'admin_audit_cohort_fixture_missing');
  let current, match;
  await ctx.step('step_1', async () => {
    current = await read(ctx, P + '/settings');
    const audit = await read(ctx, P + '/activity?action=configure&limit=50');
    match = audit.items.find(r => r.detail?.revision === String(current.revision)
      && ['platform_settings', 'triggers'].includes(r.target?.id) && r.outcome?.status === 'succeeded');
    required(match && f.actor === match.actor, 'admin_matching_audit_fixture_missing');
    await navigateSettings(session);
    const notice = session.page.getByText(/^Last platform settings change:/);
    await notice.waitFor();
    ctx.assert((await notice.textContent()).includes(match.actor_name ?? 'Name unavailable'),
      'admin_audit_actor_mismatch');
    ctx.assert(await notice.locator('time').getAttribute('datetime') === match.at, 'admin_audit_time_mismatch');
  });
  await ctx.step('step_2', async () => {
    await f.useAuditCohort('no_current_revision');
    await navigateSettings(session);
    await session.page.getByText('Last change details are not in the recent activity page.', {exact: true}).waitFor();
    ctx.assert(!(await session.page.getByText(/^Last platform settings change:/).isVisible()), 'admin_stale_audit_reused');
    const limited = await browser(ctx, 'settings_operator');
    await navigateSettings(limited);
    await limited.page.getByText('Last change details require activity-log permission.', {exact: true}).waitFor();
    const response = await roleRequest(ctx, f, 'settings_operator', 'GET', P + '/activity', undefined, {allowed: [403]});
    ctx.assert(response.status === 403, 'admin_settings_implies_audit');
    await evidence(ctx, limited);
  });
  await ctx.step('expected_result', async () => {
    const actual = await read(ctx, P + '/settings', other(ctx.am));
    ctx.assert(equal(actual, current), 'admin_audit_read_mutated_settings');
    await evidence(ctx, session);
  });
}, ['admin-fixtures', 'browser']);
