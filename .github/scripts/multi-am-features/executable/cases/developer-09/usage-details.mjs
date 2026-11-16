import {define, take, visible, equal, pick, id, poll} from './helpers.mjs';
import {setup, ready, freshResponse, apiPath, switchReadAM} from './usage-ui.mjs';
import {cells, numeric, closeNumber} from './usage-tables.mjs';
const metricNames = {cpu: 'CPU', memory: 'Memory', disk: 'Disk'};
export async function openDetail(ctx, ui, sandbox) {
  await ui.session.page.getByRole('table', {name: /Sandboxes and (measured|allocated) usage/})
    .getByRole('button', {name: sandbox.name, exact: true}).click();
  const section = ui.session.page.getByRole('region', {name: 'Sandbox usage details', exact: true});
  await visible(ctx, section.getByRole('heading', {name: sandbox.name, exact: true})); return section;
}
export const cases = [define(22, ['browser', 'admin-fixtures'], async ctx => {
  const ui = await setup(ctx); let report;
  await ctx.step('step_1', async () => {
    ctx.assert(ui.f.measured_visible === true, 'measured_fixture_required'); report = await ready(ctx, ui);
    for (const metric of ['cpu', 'memory', 'disk']) {
      const card = ui.session.page.locator('.usage-metric-card').filter({has: ui.session.page.getByRole('heading', {name: metricNames[metric], exact: true})});
      const truth = ui.f.measured_truth[metric];
      ctx.assert(equal(report.summary.measured[metric], truth), 'measured_summary_fixture');
      const text = await card.innerText();
      ctx.assert(text.includes('measured /') && text.includes('allocated now') && text.includes('Measured coverage:'), 'measured_allocated_conflated');
      ctx.assert(text.includes(truth.source) && text.includes('unknown mapping'), 'measurement_source_coverage');
    }
    ctx.assert((await ui.session.page.locator('[aria-label="Operations resource dashboard"]').innerText()).includes(`Source health: ${report.source_health}`), 'source_health_missing');
  });
  await ctx.step('step_2', async () => {
    await switchReadAM(ctx, ui.session);
    for (const [group, label] of [['total', 'Total'], ['owner', 'By owner'], ['sandbox', 'By Sandbox']]) {
      await pick(ui.session.page, 'Chart grouping', label);
      for (const metric of ['cpu', 'memory', 'disk']) {
        const chart = ui.session.page.getByRole('region', {name: `${metricNames[metric]} chart`, exact: true});
        await visible(ctx, chart);
        await poll(ctx, 'series_response', async () => {await ui.capture.flush(); return ui.capture.rows;}, rows => rows.some(r =>
          r.body?.report_id === report.report_id && r.body?.metric === metric && new URL(r.path, 'http://local.invalid').searchParams.get('group_by') === group));
        const series = ui.capture.rows.filter(r => r.body?.report_id === report.report_id && r.body?.metric === metric && new URL(r.path, 'http://local.invalid').searchParams.get('group_by') === group).at(-1).body;
        ctx.assert(equal(series.lines, ui.f.series_truth[group][metric]), 'measured_series_fixture');
        const summary = chart.getByText(`View ${metricNames[metric]} chart data`, {exact: true});
        if (await summary.locator('..').getAttribute('open') === null) await summary.click();
        for (const line of series.lines) {
          await pick(chart, 'Chart data series', line.label);
          const values = await cells(chart.getByRole('table', {name: `${metricNames[metric]} chart data`, exact: true}));
          ctx.assert(values.length === Math.min(50, line.points.length), 'measured_chart_bucket_count');
          for (let i = 0; i < values.length; i++) {
            const point = line.points[i], divisor = metric === 'cpu' ? 1 : 2 ** 30;
            ctx.assert(point.mean === null ? values[i][1] === 'Unavailable' : closeNumber(numeric(values[i][1].replace(/ (cores|GiB)$/, '')), Number(point.mean) / divisor), 'missing_measurement_presented_as_zero');
            ctx.assert(values[i][3].includes(`unknown mapping ${point.unknown_mapping_seconds}`), 'chart_unknown_mapping');
            ctx.assert(values[i][5].includes(point.reason?.replaceAll('_', ' ') ?? 'Observed'), 'chart_coverage_reason');
          }
        }
      }
    }
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(Object.values(ui.f.series_truth).some(group => Object.values(group).some(lines => lines.some(l => l.points.some(p => p.mean === null)))), 'missing_coverage_fixture');
    ctx.assert(equal(report.summary.allocated_period, ui.f.ranges['24h'].allocated_period_full), 'measured_changed_billing_allocation');
  });
}), define(23, ['browser', 'admin-fixtures'], async ctx => {
  const ui = await setup(ctx), sandbox = ui.f.detail_sandbox; let report, section, row;
  await ctx.step('step_1', async () => {
    report = await ready(ctx, ui); section = await openDetail(ctx, ui, sandbox);
    const response = await ctx.request({actor: 'admin', method: 'GET', path: `/api/v1/admin/usage/metrics/reports/${id(report.report_id)}/sandboxes/${id(sandbox.id)}`}); row = response.body;
    ctx.assert(row.sandbox_id === sandbox.id && equal(row.allocation, sandbox.allocation)
      && equal(row.allocated_period, sandbox.allocated_period), 'detail_frozen_values');
    ctx.assert((await section.innerText()).includes(row.status.replaceAll('_', ' ')), 'detail_status');
    for (const [label, identity] of [['Sandbox ID', sandbox.id], ['runtime image', sandbox.runtime_version]])
      ctx.assert(await section.getByLabel(`${label}: ${identity}`, {exact: true}).getAttribute('title') === identity, 'detail_runtime_identity');
    const totals = await section.getByRole('region', {name: 'Allocated resource-hours', exact: true}).locator('dd').allTextContents();
    ctx.assert(totals.every((value, i) => closeNumber(numeric(value), row.allocated_period[['cpu_core_hours', 'memory_gib_hours', 'disk_gib_hours'][i]])), 'detail_period_rendered');
  });
  await ctx.step('step_2', async () => {
    ctx.assert(ui.f.measured_visible === true, 'detail_measured_fixture');
    for (const metric of ['cpu', 'memory', 'disk']) await visible(ctx, section.getByRole('region', {name: `${metricNames[metric]} chart`, exact: true}));
    await switchReadAM(ctx, ui.session);
    await section.getByRole('button', {name: "View this owner's usage", exact: true}).click();
    const ownerReport = await ready(ctx, ui);
    ctx.assert(ownerReport.query.filters.owner === row.owner.subject && !ownerReport.query.filters.sandbox_id
      && ownerReport.scope.principal === ui.f.principal, 'owner_drill_scope');
    await ui.session.page.getByRole('button', {name: 'Clear filters', exact: true}).click();
    const returned = await ready(ctx, ui); ctx.assert(!returned.query.filters.owner && returned.scope.principal === ui.f.principal, 'owner_drill_return');
  });
  await ctx.step('expected_result', async () => {
    await ui.capture.flush(); const details = ui.capture.rows.filter(r => r.path.includes(`/reports/${report.report_id}/sandboxes/${sandbox.id}`));
    ctx.assert(details.length > 0 && details.every(r => r.body?.sandbox_id === sandbox.id), 'detail_response_identity');
    await ctx.request({actor: 'user', method: 'GET', path: `/api/v1/usage/metrics/reports/${id(report.report_id)}`, allowed: [403, 404]});
  });
}), define(24, ['browser', 'admin-fixtures'], async ctx => {
  const ui = await setup(ctx), faults = await take(ctx, 'usage_faults'); let report;
  await ctx.step('step_1', async () => {
    report = await ready(ctx, ui);
    ctx.assert(ui.f.history_truth.length === 2 && new Set(ui.f.history_truth.map(x => x.owner)).size === 2, 'history_cross_owner_fixture');
    for (const sandbox of ui.f.history_truth) {
      const section = await openDetail(ctx, ui, sandbox);
      const response = await freshResponse(ui.session, r => apiPath(r) === `/api/v1/usage/sandboxes/${sandbox.id}/events`, () => section.getByRole('button', {name: 'Lifecycle history', exact: true}).click());
      ctx.assert(response.status === 200 && equal(response.body.items, sandbox.pages[0].items), 'history_interval_truth');
      const history = section.getByRole('region', {name: 'Sandbox usage history', exact: true});
      const rows = await cells(history.getByRole('table', {name: 'Usage intervals', exact: true}));
      ctx.assert(rows.length === response.body.items.length && rows.every((r, i) =>
        r[0] === response.body.items[i].kind.replaceAll('_', ' ') && r[1].length > 0 && r[2].length > 0
        && numeric(r[3]) === response.body.items[i].allocation.cpu_millis && numeric(r[4]) === response.body.items[i].allocation.memory_mib
        && numeric(r[5]) === response.body.items[i].allocation.disk_gib), 'history_rendered_allocations');
      await section.getByRole('button', {name: 'Close usage details', exact: true}).click();
    }
  });
  await ctx.step('step_2', async () => {
    const sandbox = ui.f.history_truth[1], section = await openDetail(ctx, ui, sandbox);
    await section.getByRole('button', {name: 'Lifecycle history', exact: true}).click();
    await switchReadAM(ctx, ui.session);
    const next = await freshResponse(ui.session, r => apiPath(r) === `/api/v1/usage/sandboxes/${sandbox.id}/events` && new URL(r.url()).searchParams.has('cursor'),
      () => section.getByRole('button', {name: 'Next history page', exact: true}).click());
    ctx.assert(equal(next.body.items, sandbox.pages[1].items), 'history_next_page');
    const first = await freshResponse(ui.session, r => apiPath(r) === `/api/v1/usage/sandboxes/${sandbox.id}/events` && !new URL(r.url()).searchParams.has('cursor'),
      () => section.getByRole('button', {name: 'First history page', exact: true}).click());
    ctx.assert(equal(first.body.items, sandbox.pages[0].items), 'history_first_page');
    await section.getByRole('button', {name: 'Close history', exact: true}).click();
    const fault = await faults.arm({kind: 'unavailable', method: 'GET', path: `/api/v1/usage/sandboxes/${sandbox.id}/events`}); ctx.defer('history_fault', () => fault.close());
    await section.getByRole('button', {name: 'Lifecycle history', exact: true}).click();
    await visible(ctx, section.getByText('History unavailable', {exact: true}));
    await section.getByRole('button', {name: 'Close history', exact: true}).click(); await fault.release();
    await section.getByRole('button', {name: 'Lifecycle history', exact: true}).click();
    await visible(ctx, section.getByRole('table', {name: 'Usage intervals', exact: true}));
    ctx.assert(!await section.getByText('History unavailable', {exact: true}).isVisible(), 'history_error_retained_after_reopen');
  });
  await ctx.step('expected_result', async () => {
    await ctx.request({actor: 'user', method: 'GET', path: `/api/v1/usage/sandboxes/${id(ui.f.history_truth[1].id)}/events`, allowed: [403, 404]});
    ctx.assert(report.scope.admin === true, 'history_admin_scope');
  });
}), define(25, ['browser', 'admin-fixtures'], async ctx => {
  const ui = await setup(ctx); let report, panel;
  await ctx.step('step_1', async () => {
    report = await ready(ctx, ui); const summary = ui.session.page.getByText('Infrastructure diagnostics (excluded from owner totals)', {exact: true});
    await summary.click(); panel = summary.locator('..'); await visible(ctx, panel);
    ctx.assert(equal(report.summary.infrastructure, ui.f.infrastructure_truth), 'infrastructure_fixture');
  });
  await ctx.step('step_2', async () => {
    const value = report.summary.infrastructure, text = await panel.innerText();
    ctx.assert(text.includes(value.state) && text.includes(`${value.unproven_count} identities unproven`)
      && text.includes('Observed'), 'infrastructure_state_identity');
    for (let i = 0; i < 3; i++) {
      const line = panel.locator('p').filter({hasText: `${metricNames[['cpu', 'memory', 'disk'][i]]}: unassigned pool`});
      const words = await line.innerText();
      ctx.assert(words.includes('unattributed'), 'infrastructure_attribution');
      const divisor = i === 0 ? 1 : 2 ** 30;
      for (const [key, label] of [['unassigned_pool', 'unassigned pool'], ['unattributed', 'unattributed']]) {
        const amount = value[key][i];
        ctx.assert(amount === null ? words.includes(`${label} Unavailable`) : words.includes(String(Number(amount) / divisor)), 'infrastructure_missing_metric_invented');
      }
    }
  });
  await ctx.step('expected_result', async () => {
    const expected = ui.f.ranges['24h'].allocated_period;
    ctx.assert(['cpu_core_hours', 'memory_gib_hours', 'disk_gib_hours'].every(k => closeNumber(Number(report.summary.allocated_period[k]), expected[k])), 'infrastructure_in_owner_totals');
    ctx.assert(report.summary.infrastructure.unassigned_pool.some(v => Number(v) > 0)
      && report.summary.infrastructure.unattributed.some(v => v === null), 'infrastructure_nonzero_and_unknown_fixture');
  });
})];
