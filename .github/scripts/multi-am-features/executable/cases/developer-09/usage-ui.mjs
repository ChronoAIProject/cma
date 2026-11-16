import {canonicalPath, take, open, goto, captureUsage, closeAndInspect, committed, equal, visible, poll, other} from './helpers.mjs';
export const labels = {owner: 'Owner', runtime: 'Runtime version', sandbox: 'Sandbox'};
export async function setup(ctx, actor = 'admin', destination = '/operations') {
  const f = await take(ctx, 'usage'), session = await open(ctx, actor);
  const capture = captureUsage(session); ctx.defer('usage_capture', () => capture.close());
  await goto(session, destination); return {f, session, capture};
}
export async function ready(ctx, ui, range = '24h') {
  return committed(ctx, ui.session, range, ui.capture);
}
export async function choose(ctx, session, kind, value) {
  await session.page.getByRole('button', {name: new RegExp(`^${labels[kind]}:`)}).click();
  const menu = session.page.getByRole('menu', {name: `${labels[kind]} choices`, exact: true});
  await menu.getByRole('searchbox').count().then(async count => {
    if (count) await menu.getByRole('searchbox').fill(value.search ?? value.label);
    else await session.page.getByLabel(`Search ${labels[kind].toLowerCase()} choices`, {exact: true}).fill(value.search ?? value.label);
  });
  const option = menu.locator('[role="menuitemradio"]').filter({hasText: value.label});
  const exact = await option.evaluateAll((nodes, wanted) => nodes.filter(n => n.getAttribute('data-id') === wanted).length, value.id);
  if (!exact) await menu.locator('[role="menuitemradio"]').filter({hasText: value.label}).first().waitFor({state: 'visible'});
  // Select by the stable identity, not a duplicate display name.
  const target = menu.locator('[role="menuitemradio"]').and(menu.locator(`[data-id=${JSON.stringify(value.id)}]`));
  await visible(ctx, target, 'stable_option_missing'); await target.click();
}
export async function apply(ctx, ui, range = '24h') {
  await ui.session.page.getByRole('button', {name: 'Apply usage filters', exact: true}).click();
  return ready(ctx, ui, range);
}
export async function freshResponse(session, predicate, action) {
  const pending = session.page.waitForResponse(predicate, {timeout: 120000}); await action();
  const response = await pending; return {status: response.status(), body: await response.json(), request: response.request()};
}
export const apiPath = response => canonicalPath(response.url());
export async function switchReadAM(ctx, session) {
  await session.switchAM(other(ctx.am));
  ctx.defer('verify_usage_read_ams', () => closeAndInspect(session, async () => {
    const evidence = await ctx.fixture('browser').evidence(session.page);
    const reads = evidence.requests.filter(r => r.method === 'GET');
    if (![ctx.am, other(ctx.am)].every(am => reads.some(r => r.am === am && r.request_id))) throw Error('usage_actual_both_am_reads');
  }));
}
export async function inventoryPage(ctx, ui, report, action) {
  const result = await freshResponse(ui.session, r => apiPath(r).endsWith(`/reports/${report.report_id}/sandboxes`), action);
  ctx.assert(result.status === 200 && result.body.report_id === report.report_id, 'inventory_page_report_identity');
  return result.body;
}
export function assertSelection(ctx, report, selection) {
  ctx.assert(report.scope.admin === true && report.query.filters.owner === selection.owner
    && report.query.filters.sandbox_id === selection.sandbox_id && report.query.filters.runtime_version === selection.runtime_version
    && equal(report.query.filters.statuses, [selection.status]), 'filter_report_scope');
}
export async function waitOptions(ctx, capture, kind, search = '') {
  return poll(ctx, 'option_response', async () => {await capture.flush(); return capture.rows;}, rows => rows.some(r =>
    r.path === '/api/v1/admin/usage/metrics/options' && r.status === 200 && r.request_body?.kind === kind
    && r.request_body.search === search));
}
