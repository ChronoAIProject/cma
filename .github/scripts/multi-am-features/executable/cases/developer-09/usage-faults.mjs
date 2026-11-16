import {define, take, open, goto, visible, poll, pick, id, other, captureUsage, completeReport} from './helpers.mjs';
import {setup, ready, apply} from './usage-ui.mjs';
import {Blocked} from '../../runner.mjs';
const VIEW = '/api/v1/admin/usage/metrics/reports/view';
const BASE = '/api/v1/admin/usage/metrics/reports';
async function fault(ctx, f, spec) {
  const handle = await f.arm(spec); ctx.defer('restore_' + spec.kind, () => handle.close()); return handle;
}
export function backoffObserved(rows, minimumMs) {
  const attempts = rows.filter(r => [429, 503].includes(r.status));
  return attempts.length >= 1 && attempts.every(r => Number.isFinite(r.observed_ms))
    && attempts.slice(1).every((r, i) => r.observed_ms - attempts[i].observed_ms >= minimumMs);
}
export const cases = [define(15, ['browser', 'admin-fixtures'], async ctx => {
  const ui = await setup(ctx), faults = await take(ctx, 'usage_faults'); let initial, final, pendingReport;
  await ctx.step('step_1', async () => {
    initial = await ready(ctx, ui);
    const held = await fault(ctx, faults, {kind: 'hold_response', method: 'GET', path: VIEW, query: {refresh: 'true'}});
    await ui.session.page.getByRole('button', {name: 'Refresh usage', exact: true}).click();
    await poll(ctx, 'refresh_upstream', () => held.observations(), rows => rows.length > 0);
    await visible(ctx, ui.session.page.locator('.usage-report-time').filter({hasText: 'refreshing'}));
    ctx.assert(await ui.session.page.locator('[data-usage-state="committed"]').count() === 1, 'refresh_lost_committed_snapshot');
    await held.release(); await ready(ctx, ui);
    await pick(ui.session.page, 'Usage time range', 'Last hour'); final = await apply(ctx, ui, '1h');
    ctx.assert(final.report_id !== initial.report_id && final.query.to - final.query.from === 3600, 'cold_selection_scope');
  });
  await ctx.step('step_2', async () => {
    const disabled = await fault(ctx, faults, {kind: 'disabled_view', method: 'GET', path: VIEW});
    await ui.session.page.reload(); await pick(ui.session.page, 'Usage time range', 'Last 7 days');
    final = await apply(ctx, ui, '7d'); await ui.capture.flush();
    const failedView = ui.capture.rows.find(r => r.status === 501 && r.body?.code === 'report_view_disabled');
    const created = ui.capture.rows.find(r => r.method === 'POST' && r.path === BASE && r.status === 202
      && r.body?.query?.to - r.body?.query?.from === 7 * 86400);
    ctx.assert(!!failedView && !!created && created.observed_ms > failedView.observed_ms, 'typed_fallback_or_pending_not_observed');
    pendingReport = created.body.report_id;
    ctx.assert(ui.capture.rows.some(r => r.method === 'GET' && r.path === `${BASE}/${pendingReport}` && r.status === 200), 'fallback_exact_descriptor_poll');
    await disabled.release();
    for (const kind of ['busy', 'unavailable']) {
      const handle = await fault(ctx, faults, {kind, method: 'GET', path: VIEW, retry_after_seconds: 2});
      const start = performance.now(); await ui.session.page.reload();
      await visible(ctx, ui.session.page.getByText(/Usage reporting is busy|Usage reporting is unavailable|Retrying usage automatically/).first());
      const rows = await poll(ctx, 'backoff_attempts', async () => {await ui.capture.flush(); return ui.capture.rows.filter(r => r.observed_ms >= start && r.path.startsWith(VIEW));},
        rows => rows.filter(r => [429, 503].includes(r.status)).length >= 2);
      ctx.assert(backoffObserved(rows, 1900), 'usage_retry_storm');
      await handle.release(); final = await ready(ctx, ui, '7d');
      ctx.assert(final.query.to - final.query.from === 7 * 86400 && final.scope.principal === ui.f.principal, 'backoff_changed_scope');
    }
    const expired = await fault(ctx, faults, {kind: 'expire_report', method: 'GET', path: `${BASE}/${final.report_id}/sandboxes`});
    await pick(ui.session.page, 'Sort usage inventory', 'Owner');
    await visible(ctx, ui.session.page.getByText('Usage report expired', {exact: true}));
    ctx.assert(await ui.session.page.locator('[data-usage-state="error"]').count() === 1, 'expired_report_shown_complete');
    await expired.release(); await ui.session.page.getByRole('button', {name: 'Refresh usage report', exact: true}).click();
    final = await ready(ctx, ui, '7d'); ctx.assert(final.report_id !== pendingReport, 'expired_report_not_replaced');
  });
  await ctx.step('step_3', async () => {
    const held = await fault(ctx, faults, {kind: 'hold_response', method: 'GET', path: VIEW, query: {window: '30d'}});
    await pick(ui.session.page, 'Usage time range', 'Last 30 days');
    await ui.session.page.getByRole('button', {name: 'Apply usage filters', exact: true}).click();
    await poll(ctx, 'obsolete_pending', () => held.observations(), rows => rows.length > 0);
    await visible(ctx, ui.session.page.getByText(/This is the last committed last 7 days report/));
    await pick(ui.session.page, 'Usage time range', 'Last 24 hours');
    await ui.session.page.getByRole('button', {name: 'Apply usage filters', exact: true}).click();
    await goto(ui.session, '/'); await held.release();
    await ui.session.switchAM(other(ctx.am)); await goto(ui.session, '/operations?range=24h'); final = await ready(ctx, ui);
    ctx.assert(final.query.to - final.query.from === 86400 && final.scope.principal === ui.f.principal, 'obsolete_response_replaced_current_selection');
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(completeReport(final, ui.f.ranges['24h'], ui.f.principal), 'retry_final_not_complete');
    await ctx.observe('usage_fault_receipts', async () => ui.capture.rows.map(r => ({method: r.method, status: r.status,
      request_id: r.request_id, report_id: r.body?.report_id ?? r.body?.report?.report_id, observed_ms: r.observed_ms})));
    ctx.assert(await ui.session.page.locator('[data-usage-state="preview"], [data-usage-state="error"]').count() === 0, 'final_stale_error');
  });
}), define(29, ['browser', 'admin-fixtures'], async ctx => {
  const identities = await take(ctx, 'usage_identity'), faults = await take(ctx, 'usage_faults');
  const session = await open(ctx, 'admin_session');
  if ((await ctx.fixture('browser').evidence(session.page)).authentication !== 'browser_session') throw new Blocked('real_browser_session_required');
  const capture = captureUsage(session); ctx.defer('identity_usage_capture', () => capture.close());
  let adminReport, personalReport, held;
  await ctx.step('step_1', async () => {
    await goto(session, '/operations');
    await poll(ctx, 'admin_identity_report', async () => {await capture.flush(); return capture.rows;}, rows => rows.some(r => (r.body?.report ?? r.body)?.status === 'ready'));
    adminReport = capture.rows.map(r => r.body?.report ?? r.body).find(r => r?.status === 'ready' && r.scope?.admin);
    ctx.assert(adminReport?.scope.principal === identities.admin_principal, 'admin_session_principal');
    await session.page.getByRole('button', {name: /^Account menu for /}).click();
    await session.page.getByRole('menuitem', {name: 'Usage', exact: true}).click();
    const dialog = session.page.getByRole('dialog', {name: 'Your usage', exact: true}); await visible(ctx, dialog);
    await poll(ctx, 'personal_identity_report', async () => {await capture.flush(); return capture.rows;}, rows => rows.some(r => (r.body?.report ?? r.body)?.status === 'ready' && (r.body?.report ?? r.body)?.scope?.admin === false));
    personalReport = capture.rows.map(r => r.body?.report ?? r.body).find(r => r?.status === 'ready' && r.scope?.admin === false);
    ctx.assert(personalReport.scope.principal === identities.admin_principal && personalReport.report_id !== adminReport.report_id, 'personal_admin_scope_mixed');
    held = await fault(ctx, faults, {kind: 'hold_response', method: 'GET', path: '/api/v1/usage/metrics/reports/view', query: {refresh: 'true'}});
    await dialog.getByRole('button', {name: 'Refresh usage', exact: true}).click();
    await poll(ctx, 'personal_pending_identity', () => held.observations(), rows => rows.length > 0);
    await dialog.getByRole('button', {name: 'Close', exact: true}).click();
    await session.page.getByRole('button', {name: /^Account menu for /}).click();
    await session.page.getByRole('menuitem', {name: 'Sign out', exact: true}).click();
    await session.page.getByRole('button', {name: /^Account menu for /}).waitFor({state: 'hidden'});
    ctx.assert(await session.page.locator('[aria-label="Your usage resources"], [aria-label="Operations resource dashboard"]').count() === 0, 'signed_out_report_retained');
    // Real provider login through the existing Page; no bearer/storage injection.
    await identities.login(session, 'other_session'); await held.release();
  });
  await ctx.step('step_2', async () => {
    await session.switchAM(other(ctx.am)); await goto(session, '/operations');
    await visible(ctx, session.page.getByRole('heading', {name: 'Operations unavailable', exact: true}));
    for (const [prefix, report] of [['admin/', adminReport], ['', personalReport]]) {
      const denied = await ctx.request({actor: 'other_session', am: other(ctx.am), method: 'GET',
        path: `/api/v1/${prefix}usage/metrics/reports/${id(report.report_id)}`, allowed: [403, 404]});
      ctx.assert(!JSON.stringify(denied.body).includes(report.report_id), 'previous_principal_report_disclosed');
    }
    const start = performance.now();
    await session.page.getByRole('button', {name: /^Account menu for /}).click();
    await session.page.getByRole('menuitem', {name: 'Usage', exact: true}).click();
    await poll(ctx, 'new_personal_report', async () => {await capture.flush(); return capture.rows.filter(r => r.observed_ms >= start);}, rows => rows.some(r => {
      const value = r.body?.report ?? r.body; return value?.status === 'ready' && value.scope?.principal === identities.other_principal && value.scope.admin === false;
    }));
    const text = await session.page.getByRole('dialog', {name: 'Your usage', exact: true}).innerText();
    ctx.assert(identities.old_private_markers.every(m => !text.includes(m)), 'late_report_populated_new_principal');
  });
  await ctx.step('expected_result', async () => {
    const report = capture.rows.map(r => r.body?.report ?? r.body).filter(r => r?.status === 'ready' && r.scope?.principal === identities.other_principal).at(-1);
    ctx.assert(report && report.scope.admin === false && report.query.to - report.query.from === 86400
      && ![adminReport.report_id, personalReport.report_id].includes(report.report_id), 'identity_report_range_or_scope');
  });
})];
