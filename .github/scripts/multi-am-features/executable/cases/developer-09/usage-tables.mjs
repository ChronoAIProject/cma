import {define, equal, pick, visible, poll} from './helpers.mjs';
import {setup, ready, choose, apply, inventoryPage, switchReadAM} from './usage-ui.mjs';
const sorts = [['Sandbox name', 'name'], ['Owner', 'owner'], ['Status', 'status'], ['Last activity', 'last_activity_at'],
  ['Allocated CPU', 'allocated_cpu_core_hours'], ['Allocated memory', 'allocated_memory_gib_hours'], ['Allocated disk', 'allocated_disk_gib_hours']];
const measured = [['Measured CPU', 'cpu_mean_cores'], ['Measured memory', 'memory_mean_bytes'], ['Measured disk', 'disk_latest_bytes']];
export const numeric = text => Number(text.trim().replaceAll(',', ''));
export function closeNumber(actual, expected) { return Number.isFinite(actual) && Number.isFinite(Number(expected)) && Math.abs(actual - Number(expected)) <= 0.001; }
export async function cells(table) { return table.locator('tbody tr').evaluateAll(rows => rows.map(row => [...row.querySelectorAll('td')].map(cell => cell.textContent.trim())).filter(row => row.length > 1)); }
export const cases = [define(20, ['browser', 'admin-fixtures'], async ctx => {
  const ui = await setup(ctx); let report, last;
  async function sort(label, key) {
    for (const reverse of [false, true]) {
      last = await inventoryPage(ctx, ui, report, () => reverse
        ? ui.session.page.getByRole('button', {name: /^Sort (ascending|descending)$/}).click()
        : pick(ui.session.page, 'Sort usage inventory', label));
      const direction = new URL(ui.session.page.url()).searchParams.get('order') === 'asc' ? 'asc' : 'desc';
      ctx.assert(equal(last.items.map(x => x.sandbox_id), ui.f.expected_sort_orders[key][direction].slice(0, 50)), 'inventory_sort_order');
      const table = ui.session.page.getByRole('table', {name: /Sandboxes and (measured|allocated) usage/});
      const names = await table.locator('.usage-inventory-name button').allTextContents();
      ctx.assert(equal(names, last.items.map(x => x.name || 'Unnamed Sandbox')), 'sorted_rendered_rows');
    }
  }
  await ctx.step('step_1', async () => { report = await ready(ctx, ui); for (const [label, key] of sorts) await sort(label, key); });
  await ctx.step('step_2', async () => {
    ctx.assert(ui.f.measured_visible === true, 'measured_sort_fixture');
    for (const [label, key] of measured) await sort(label, key);
  });
  await ctx.step('step_3', async () => {
    const first = last, all = first.items.map(x => x.sandbox_id); let page = first, count = 1;
    await switchReadAM(ctx, ui.session);
    while (page.next) {
      ctx.assert(++count <= 100, 'inventory_page_bound');
      page = await inventoryPage(ctx, ui, report, () => ui.session.page.getByRole('button', {name: 'Next usage page', exact: true}).click());
      for (const item of page.items) {ctx.assert(!all.includes(item.sandbox_id), 'inventory_duplicate'); all.push(item.sandbox_id);}
    }
    ctx.assert(count > 1 && equal([...all].sort(), [...ui.f.sandbox_ids].sort()), 'inventory_complete_cohort');
    while (count-- > 1) page = await inventoryPage(ctx, ui, report, () => ui.session.page.getByRole('button', {name: 'Previous usage page', exact: true}).click());
    ctx.assert(equal(page.items, first.items), 'inventory_previous_page');
    await choose(ctx, ui.session, 'owner', ui.f.empty_owner); report = await apply(ctx, ui);
    await visible(ctx, ui.session.page.getByText('No Sandboxes match this selection.', {exact: true}));
    ctx.assert(Number(report.summary.sandbox_count) === 0, 'empty_filter_not_empty');
  });
  await ctx.step('expected_result', async () => {
    await ui.capture.flush(); const pages = ui.capture.rows.filter(r => /\/reports\/[^/]+\/sandboxes\?/.test(r.path) && r.status === 200);
    ctx.assert(pages.length >= 22 && pages.every(p => p.body?.report_id && p.path.includes('/' + p.body.report_id + '/')), 'page_identity_contract');
  });
}), define(21, ['browser', 'admin-fixtures'], async ctx => {
  const ui = await setup(ctx); let report; const observed = {};
  await ctx.step('step_1', async () => {
    report = await ready(ctx, ui);
    for (const metric of ['cpu', 'memory', 'disk']) {
      const label = {cpu: 'CPU', memory: 'Memory', disk: 'Disk'}[metric];
      await pick(ui.session.page, 'Allocation chart resource', label);
      const section = ui.session.page.getByRole('region', {name: 'Allocation comparison chart', exact: true});
      await visible(ctx, section.getByText(new RegExp('Sandboxes on this page')));
      const summary = section.getByText('View allocation chart data', {exact: true});
      if (await summary.locator('..').getAttribute('open') === null) await summary.click();
      const table = section.getByRole('table', {name: 'Allocation chart data', exact: true});
      const values = await cells(table), truth = ui.f.allocation_truth[metric].sandboxes;
      ctx.assert(values.length === truth.length && values.every((r, i) => r[0] === truth[i].name
        && closeNumber(numeric(r[1]), truth[i].value) && r[2] === (truth[i].complete ? 'Complete' : 'Partial')), 'sandbox_allocation_chart_values');
      ctx.assert((await section.locator('.usage-chart-unit').innerText()).includes(metric === 'cpu' ? 'core-hours' : 'GiB-hours'), 'allocation_units');
    }
  });
  await ctx.step('step_2', async () => {
    await switchReadAM(ctx, ui.session);
    await pick(ui.session.page, 'Allocation chart grouping', 'By owner');
    for (const metric of ['cpu', 'memory', 'disk']) {
      await pick(ui.session.page, 'Allocation chart resource', {cpu: 'CPU', memory: 'Memory', disk: 'Disk'}[metric]);
      await pick(ui.session.page, 'Owners shown', 'All');
      await poll(ctx, 'owner_load_all', async () => {
        const more = ui.session.page.getByRole('button', {name: 'Load more owners', exact: true});
        if (await more.isVisible() && await more.isEnabled()) await more.click();
        return ui.session.page.getByText(/All owners loaded\./).isVisible();
      }, done => done);
      const summary = ui.session.page.getByText('View owner allocation chart data', {exact: true});
      if (await summary.locator('..').getAttribute('open') === null) await summary.click();
      const all = [], table = ui.session.page.getByRole('table', {name: 'Owner allocation chart data', exact: true});
      for (let n = 0; n < 100; n++) {
        all.push(...await cells(table)); const next = ui.session.page.getByRole('button', {name: 'Next owner values', exact: true});
        if (await next.isDisabled()) break; await next.click(); ctx.assert(n < 99, 'owner_table_page_bound');
      }
      const truth = ui.f.allocation_truth[metric].owners;
      ctx.assert(all.length > 20 && all.length === truth.length && all.every((r, i) => r[0].includes(truth[i].label)
        && closeNumber(numeric(r[1]), truth[i].allocated_hours) && numeric(r[2]) === truth[i].sandbox_count
        && r[3] === (truth[i].complete ? 'Complete' : 'Partial')), 'owner_allocation_ranking_values');
      observed[metric] = all; await ui.session.page.getByRole('button', {name: 'Previous owner values', exact: true}).click();
      ctx.assert((await cells(table)).length > 0, 'owner_previous_values');
    }
  });
  await ctx.step('step_3', async () => {
    for (const [metric, field] of [['cpu', 'cpu_core_hours'], ['memory', 'memory_gib_hours'], ['disk', 'disk_gib_hours']]) {
      const total = ui.f.allocation_truth[metric].owners.reduce((sum, r) => sum + Number(r.allocated_hours), 0);
      ctx.assert(closeNumber(total, report.summary.allocated_period[field]), 'owner_totals_report');
      ctx.assert(observed[metric].some(r => r[3] === 'Partial'), 'partial_history_missing');
    }
    await visible(ctx, ui.session.page.getByText(/^Partial history: bars show recorded allocation only/));
  });
  await ctx.step('expected_result', async () => {
    await ui.capture.flush(); const ownerPages = ui.capture.rows.filter(r => r.path.includes('/owner-allocations') && r.status === 200);
    ctx.assert(ownerPages.length >= 3 && ownerPages.every(r => r.body.report_id === report.report_id), 'owner_chart_report_identity');
  });
})];
