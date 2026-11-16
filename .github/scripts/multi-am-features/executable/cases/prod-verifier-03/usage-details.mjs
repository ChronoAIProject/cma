import {define, openBrowser, account, visible, uiReceipt, invoke, digest} from './helpers.mjs';

const dashboard = page => page.locator('[data-usage-requested-range]');
const dialog = page => page.getByRole('dialog', {name: 'Your usage', exact: true});
const apply = page => page.getByRole('button', {name: 'Apply usage filters', exact: true}).click();
const choose = async (page, label, value) => {
  await page.getByRole('button', {name: label, exact: true}).click();
  await page.getByRole('menuitemradio', {name: value, exact: true}).click();
};
async function usage(ctx, options) {
  const session = await openBrowser(ctx, 'user', {usage: 'populated', ...options});
  const records = [], pending = [];
  session.page.on('response', response => {
    const url = new URL(response.url());
    if (!url.pathname.startsWith('/api/v1/usage/')) return;
    const record = {path: url.pathname, query: Object.fromEntries(url.searchParams),
      method: response.request().method(), status: response.status()}; records.push(record);
    pending.push(response.json().then(body => {record.body = body;}, () => {record.body = null;}));
  });
  session.records = records; session.flush = async () => {await invoke(session, 'networkIdle'); await Promise.all(pending);};
  await account(session.page);
  await session.page.getByRole('menuitem', {name: 'Usage', exact: true}).click();
  await visible(ctx, dialog(session.page), 'usage_dialog_open');
  return session;
}
async function ready(ctx, session) {
  await session.page.waitForFunction(() => {
    const node = document.querySelector('[data-usage-requested-range]');
    return node?.getAttribute('data-usage-state') === 'committed' && !node.querySelector('[aria-busy="true"]');
  }, undefined, {timeout: 90000});
  await session.flush();
  const report = session.records.map(record => record.body?.report ?? record.body)
    .filter(value => value?.status === 'ready' && value.summary).at(-1);
  ctx.assert(report?.scope.admin === false && report.scope.principal === session.identity.subject,
    'usage_ready_personal_identity');
  return report;
}

export const usageDetailCases = [
  define(15, 'browser', ['browser', 'eg_browser'], async ctx => {
    let s, report;
    await ctx.step('step_1', async () => {
      s = await usage(ctx, {usageFilters: true}); await ready(ctx, s);
      await s.page.getByRole('button', {name: /^Sandbox:/}).click();
      await s.page.getByLabel('Search sandbox choices', {exact: true}).fill(s.usage.sandboxName);
      await s.page.getByRole('button', {name: 'Search sandbox choices', exact: true}).click();
      await choose(s.page, 'Choose sandbox', `${s.usage.sandboxName} (${s.usage.sandboxId})`);
      ctx.assert(await s.page.getByRole('button', {name: /^Agent:/}).count() === 0,
        'usage_agent_waits_for_matching_report');
      await s.page.getByLabel('Usage Sandbox search', {exact: true}).fill(s.usage.sandboxId);
      await apply(s.page); report = await ready(ctx, s);
      ctx.assert(report.query.filters.sandbox_id === s.usage.sandboxId, 'usage_selected_sandbox_applied');
      const rows = s.records.filter(r => r.path.endsWith(`/reports/${report.report_id}/sandboxes`)).at(-1)?.body.items;
      ctx.assert(rows?.length > 0 && rows.every(row => row.sandbox_id === s.usage.sandboxId
        && row.owner.subject === s.identity.subject), 'usage_search_exact_owned_sandbox');
    });
    await ctx.step('step_2', async () => {
      await s.page.getByRole('button', {name: /^Agent:/}).click();
      await choose(s.page, 'Usage Agent', s.usage.agentName); await apply(s.page);
      report = await ready(ctx, s);
      ctx.assert(report.query.filters.sandbox_id === s.usage.sandboxId, 'usage_agent_resolves_shared_sandbox');
      await visible(ctx, s.page.getByText(/CPU, memory and disk are shared with its other Agents\./),
        'usage_agent_shared_resources_explained');
      await s.page.getByRole('button', {name: /^Sandbox:/}).click();
      await s.page.getByRole('button', {name: 'All my Sandboxes', exact: true}).click();
      await s.page.getByLabel('Usage Sandbox search', {exact: true}).fill(''); await apply(s.page);
      const cleared = await ready(ctx, s);
      ctx.assert(!cleared.query.filters.sandbox_id, 'usage_cleared_sandbox_filter');
      await s.page.getByLabel('Usage Sandbox search', {exact: true}).fill(s.usage.sandboxName); await apply(s.page);
      report = await ready(ctx, s);
      const rows = s.records.filter(r => r.path.endsWith(`/reports/${report.report_id}/sandboxes`)).at(-1)?.body.items;
      ctx.assert(rows?.some(row => row.sandbox_id === s.usage.sandboxId), 'usage_name_search_reapplied');
    });
    await ctx.step('expected_result', async () => {
      for (const label of ['Usage Sandbox status', 'Usage runtime version'])
        ctx.assert(await s.page.getByLabel(label, {exact: true}).count() === 0, 'usage_admin_control_absent');
      ctx.assert(await s.page.getByRole('button', {name: /^Owner:/}).count() === 0, 'usage_admin_owner_absent');
      ctx.assert(s.records.filter(r => r.body?.items?.[0]?.owner).every(r =>
        r.body.items.every(item => item.owner.subject === s.identity.subject)), 'usage_every_result_personal');
      await ctx.observe('usage_filter_report', async () => ({report_sha256: digest(report)})); await uiReceipt(ctx, s.page);
    });
  }),
  define(16, 'browser', ['browser', 'eg_browser'], async ctx => {
    let s, report;
    const reportIds = new Set();
    await ctx.step('step_1', async () => {
      s = await usage(ctx, {usageDetails: true, measuredUsage: true}); report = await ready(ctx, s);
      await visible(ctx, s.page.getByRole('region', {name: 'Allocated resource-hours', exact: true}), 'usage_totals_present');
      const values = await s.page.getByRole('region', {name: 'Allocated resource-hours', exact: true}).locator('dd').allTextContents();
      for (const [i, key] of ['cpu_core_hours', 'memory_gib_hours', 'disk_gib_hours'].entries()) {
        const value = report.summary.allocated_period[key];
        const formatted = typeof value === 'string' ? BigInt(value).toLocaleString('en-SG')
          : value.toLocaleString('en-SG', {maximumFractionDigits: 3});
        ctx.assert(values[i]?.trim() === formatted, 'usage_totals_match_committed_report');
      }
      await visible(ctx, s.page.getByRole('region', {name: 'Measured resource charts'}), 'usage_measured_chart_present');
    });
    await ctx.step('step_2', async () => {
      for (const metric of ['CPU', 'Memory working set', 'Home PVC disk']) {
        await s.page.getByRole('button', {name: metric, exact: true}).click(); await s.flush();
        await visible(ctx, s.page.getByRole('region', {name: metric + ' chart', exact: true}), 'usage_selected_measured_chart');
        ctx.assert(await s.page.getByRole('button', {name: metric, exact: true}).getAttribute('aria-pressed') === 'true',
          'usage_metric_selected');
      }
      await choose(s.page, 'Sort usage inventory', 'Allocated CPU'); report = await ready(ctx, s);
      reportIds.add(report.report_id);
      const path = `/api/v1/usage/metrics/reports/${report.report_id}/sandboxes`;
      const first = s.records.filter(r => r.path === path && r.query.sort === 'allocated_cpu_core_hours').at(-1);
      ctx.assert(first?.body.items.length > 0 && first.body.next, 'usage_fixture_second_page_available');
      const cpu = first.body.items.map(row => Number(row.allocated_period.cpu_core_hours));
      ctx.assert(cpu.every((value, index) => index === 0 || (first.query.order === 'desc'
        ? value <= cpu[index - 1] : value >= cpu[index - 1])), 'usage_sorted_allocated_cpu');
      await s.page.getByRole('button', {name: 'Next usage page', exact: true}).click(); await s.flush();
      const second = s.records.filter(r => r.path === path && r.query.cursor === first.body.next).at(-1);
      ctx.assert(second?.body.items.length > 0 && second.body.items.every(row =>
        !first.body.items.some(previous => previous.sandbox_id === row.sandbox_id)), 'usage_pages_do_not_duplicate');
      await s.page.getByRole('button', {name: 'Previous usage page', exact: true}).click(); await s.flush();
      await s.page.getByRole('button', {name: s.usage.detailSandboxName, exact: true}).click(); await s.flush();
      const detail = s.records.filter(r => r.path === path + '/' + encodeURIComponent(s.usage.detailSandboxId)).at(-1)?.body;
      ctx.assert(detail?.sandbox_id === s.usage.detailSandboxId && detail.owner.subject === s.identity.subject,
        'usage_detail_same_report_owner');
      const region = s.page.getByRole('region', {name: 'Sandbox usage details', exact: true});
      await visible(ctx, region, 'usage_detail_visible');
      ctx.assert(await region.getByText(/Unavailable|No measured samples/).count() > 0,
        'usage_missing_measurements_not_zero');
      await region.getByRole('button', {name: 'Lifecycle history', exact: true}).click(); await s.flush();
      await visible(ctx, s.page.getByRole('region', {name: 'Sandbox usage history', exact: true}), 'usage_allocation_history_visible');
      const history = s.records.find(r => r.path.endsWith('/' + s.usage.detailSandboxId + '/events'))?.body;
      ctx.assert(history?.items.length > 0 && s.usage.historyTransactionIds.every(id =>
        history.items.some(item => item.transaction_id === id)), 'usage_actual_allocation_history');
    });
    await ctx.step('expected_result', async () => {
      const series = s.records.filter(r => r.path.includes(`/reports/${report.report_id}/`) && r.path.endsWith('/series'));
      ctx.assert(series.length >= 3 && series.every(r => r.status === 200), 'usage_charts_same_completed_report');
      ctx.assert(reportIds.size === 1 && await dashboard(s.page).getAttribute('data-usage-state') === 'committed',
        'usage_detail_report_stable');
      await ctx.observe('usage_detail_receipt', async () => ({report_sha256: digest(report), series: series.map(r => digest(r.body))}));
      await uiReceipt(ctx, s.page);
    });
  }),
  define(17, 'browser', ['browser', 'eg_browser'], async ctx => {
    let s;
    await ctx.step('step_1', async () => {
      s = await usage(ctx, {usageLifecycle: true}); await ready(ctx, s);
      const before = s.records.length;
      await s.page.getByRole('button', {name: 'Refresh usage', exact: true}).click(); await ready(ctx, s);
      ctx.assert(s.records.length > before, 'usage_refresh_observed_network');
      await dialog(s.page).getByRole('button', {name: 'Close', exact: true}).click(); await s.flush();
      const quiet = await invoke(s, 'usageObservationBarrier', {state: 'closed'});
      ctx.assert(quiet.pendingRequests === 0 && quiet.backgroundPolls === 0, 'usage_closed_stops_observation');
      await account(s.page); await s.page.getByRole('menuitem', {name: 'Usage', exact: true}).click();
      await ready(ctx, s); ctx.assert(await dialog(s.page).isVisible(), 'usage_reopened_loads');
    });
    await ctx.step('step_2', async () => {
      for (const fault of ['report_view_disabled', 'missing_route', 'pending', 'expired', 'forbidden', 'transient', 'other_501']) {
        const local = await usage(ctx, {usageFault: fault});
        await invoke(local, 'awaitUsageFault', {fault});
        if (['report_view_disabled', 'missing_route', 'pending'].includes(fault)) await ready(ctx, local);
        else {
          await local.flush();
          await visible(ctx, local.page.getByText(fault === 'expired' ? 'Usage report expired'
            : fault === 'forbidden' ? 'Usage access unavailable'
              : fault === 'transient' ? 'Usage reporting is unavailable' : 'Could not load usage', {exact: true}),
          'usage_fault_explicit');
        }
        await local.flush();
        const posts = local.records.filter(r => r.path.endsWith('/metrics/reports') && r.method === 'POST');
        if (['report_view_disabled', 'missing_route'].includes(fault)) {
          ctx.assert(posts.length === 1 && posts[0].body.report_id, 'usage_supported_fallback_one_report');
          const reads = local.records.filter(r => r.method === 'GET' && /\/reports\/[^/]+$/.test(r.path));
          ctx.assert(reads.length > 0 && reads.every(r => r.path.endsWith('/' + posts[0].body.report_id)),
            'usage_fallback_exact_report_poll');
        } else if (['forbidden', 'transient', 'other_501'].includes(fault))
          ctx.assert(posts.length === 0, 'usage_failure_no_unsupported_fallback');
        if (['forbidden', 'expired'].includes(fault)) ctx.assert(await local.page.locator('.usage-report').count() === 0,
          'usage_identity_expiry_clears_report');
        if (fault === 'pending') {
          const observed = await invoke(local, 'usagePollLedger');
          ctx.assert(observed.pendingSeen === true && observed.reportIds.length === 1
            && observed.reportIds[0] === observed.createdReportId, 'usage_pending_exact_identity');
        }
        await dialog(local.page).getByRole('button', {name: 'Close', exact: true}).click();
        const quiet = await invoke(local, 'usageObservationBarrier', {state: 'hidden'});
        ctx.assert(quiet.pendingRequests === 0 && quiet.backgroundPolls === 0, 'usage_hidden_stops_fault_observation');
        await uiReceipt(ctx, local.page);
      }
    });
    await ctx.step('expected_result', async () => {
      ctx.assert(await dashboard(s.page).getAttribute('data-usage-state') === 'committed', 'usage_original_session_healthy');
      await uiReceipt(ctx, s.page);
    });
  }),
];
