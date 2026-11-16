import {setTimeout as delay} from 'node:timers/promises';
import {browser, request, other, id, check, equal} from './common.mjs';
import {captureMeasured} from './measured-capture.mjs';
import {selectedMeasured, cpuGap, measuredSeries, reportProof, allocationKeys, capacityKeys, displayQuantity, metricTime} from './measured-proof.mjs';

export async function waitMeasured(ctx, operation, code) {
  const deadline = performance.now() + Math.min(30000, ctx.remainingMs());
  for (;;) {
    ctx.signal.throwIfAborted();
    const value = await operation();
    if (value) return value;
    check(ctx.remainingMs() > 1000 && performance.now() < deadline, code);
    await delay(Math.min(200, deadline - performance.now()), undefined, {signal: ctx.signal});
  }
}

export async function measuredReport(ctx, f, actor, enabled) {
  const am = other(ctx.am), session = await browser(ctx, actor, am);
  const capture = captureMeasured(ctx, session, actor, am, enabled);
  const params = new URLSearchParams({range: 'custom', from: new Date(f.range.from * 1000).toISOString(),
    to: new Date(f.range.to * 1000).toISOString()});
  if (actor === 'admin') params.set('tab', 'sandboxes');
  await session.page.goto(new URL((actor === 'admin' ? '/operations?' : '/?') + params, session.origin).href);
  if (actor === 'user') {
    await session.page.getByRole('button', {name: /^Account menu for /}).click();
    await session.page.getByRole('menuitem', {name: 'Usage', exact: true}).click();
    await session.page.getByRole('dialog', {name: 'Your usage', exact: true}).waitFor();
  }
  const dashboard = session.page.locator('[aria-label="' + (actor === 'admin' ? 'Operations resource dashboard' : 'Your usage resources') + '"]');
  const table = dashboard.getByRole('table', {name: /Sandboxes and (measured|allocated) usage/});
  const selected = await waitMeasured(ctx, async () => {
    await capture.flush();
    if (await dashboard.getAttribute('data-usage-state') !== 'committed'
      || await dashboard.getAttribute('data-usage-requested-range') !== 'custom'
      || !await table.isVisible() || await table.getAttribute('aria-busy') === 'true') return;
    return selectedMeasured(capture.rows, actor, f, capture.base);
  }, 'admin_measured_report_not_committed');
  const {report, inventory} = selected;
  const auth = capture.rows.filter(r => r.path === '/auth/session' && r.status === 200).at(-1);
  ctx.assert(auth?.body.authenticated === true && auth.body.subject === f.scopes[actor].subject
    && auth.body.usage_reporting?.measured_visible === enabled, 'admin_measured_session_policy');
  const allocated = dashboard.getByRole('region', {name: 'Allocated resource-hours', exact: true});
  ctx.assert(await allocated.isVisible() && await dashboard.getByRole('region', {name: 'Allocated capacity now', exact: true}).isVisible(),
    'admin_measured_allocation_hidden');
  ctx.assert(equal(await allocated.locator('.usage-resource-card > dd').allTextContents(),
    allocationKeys.map(k => displayQuantity(report.summary.allocated_period[k]))), 'admin_measured_rendered_allocation');
  ctx.assert(equal(await allocated.locator('.usage-resource-now dd').allTextContents(),
    capacityKeys.map((k, i) => displayQuantity(report.summary.total_allocated_capacity[k], i ? 2 ** 30 : 1)
      + (i ? ' GiB' : ' cores'))), 'admin_measured_rendered_capacity');
  ctx.assert(equal(await table.locator('.usage-inventory-name button').allTextContents(),
    inventory.items.map(r => r.name || 'Unnamed Sandbox')), 'admin_measured_rendered_inventory');
  const peer = await request(ctx, 'GET', capture.base + '/' + id(report.report_id), undefined, {actor, am: other(am)});
  reportProof(peer.body, actor, f);
  ctx.assert(equal(peer.body, report), 'admin_measured_peer_report');
  for (const label of ['CPU', 'Memory working set', 'Home PVC disk']) {
    const panel = dashboard.getByRole('region', {name: label, exact: true});
    if (enabled) await panel.waitFor();
    ctx.assert(await panel.isVisible() === enabled, 'admin_measured_summary_visibility');
  }
  ctx.assert(await table.locator('.usage-inventory-measured').count() === (enabled ? inventory.items.length * 3 : 0),
    'admin_measured_inventory_visibility');
  const measured = dashboard.getByRole('region', {name: 'Measured resource charts', exact: true});
  if (enabled) {
    await measured.waitFor();
    for (const [metric, label] of (actor === 'admin' ? [['cpu', 'CPU'], ['memory', 'Memory working set'], ['disk', 'Home PVC disk']]
      : [['cpu', 'CPU']])) {
      await waitMeasured(ctx, async () => {await capture.flush(); return measuredSeries(capture.rows, report.report_id, capture.base, metric);},
        'admin_measured_series_missing');
      await measured.getByRole('region', {name: label + ' chart', exact: true}).waitFor();
    }
    const gap = await waitMeasured(ctx, async () => {await capture.flush(); return cpuGap(capture.rows, report.report_id, capture.base);},
      'admin_measured_series_missing');
    const chart = measured.getByRole('region', {name: 'CPU chart', exact: true});
    await chart.getByText('View CPU chart data', {exact: true}).click();
    const data = chart.getByRole('table', {name: 'CPU chart data', exact: true});
    await data.waitFor();
    for (let page = 0; page < Math.floor(gap.index / 50); page++)
      await chart.getByRole('button', {name: 'Next chart data', exact: true}).click();
    const cells = data.getByRole('row').nth(gap.index % 50 + 1).getByRole('cell');
    ctx.assert(await cells.nth(0).innerText() === metricTime(gap.point.from) + ' – ' + metricTime(gap.point.to),
      'admin_measured_gap_row');
    ctx.assert(await cells.nth(1).innerText() === 'Unavailable', 'admin_measured_gap_fabricated_zero');
    ctx.assert((await cells.nth(5).innerText()).includes(gap.point.reason.replaceAll('_', ' ')), 'admin_measured_gap_reason');
    await ctx.observe('admin_measured_gap', async () => ({actor, report_id: report.report_id, ...gap}));
  }
  ctx.assert(await measured.isVisible() === enabled, 'admin_measured_visibility_mismatch');
  await capture.proof();
  await capture.close();
  return ctx.observe('admin_measured_completed_report', async () => ({actor, am, enabled, report, inventory}));
}
