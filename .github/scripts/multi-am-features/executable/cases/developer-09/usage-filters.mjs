import {canonicalPath, define, take, visible, equal, pick} from './helpers.mjs';
import {labels, setup, choose, ready, apply, assertSelection, waitOptions, switchReadAM} from './usage-ui.mjs';
const choices = [[16, 'owner'], [17, 'runtime'], [18, 'sandbox']].map(([number, kind]) => define(number, ['browser', 'admin-fixtures'], async ctx => {
  const faults = await take(ctx, 'usage_faults');
  const hold = await faults.arm({kind: 'hold_response', method: 'GET', path: '/api/v1/admin/usage/metrics/reports/view'});
  ctx.defer('pending_view', () => hold.close());
  const ui = await setup(ctx); let first;
  await ctx.step('step_1', async () => {
    await ui.session.page.getByRole('button', {name: new RegExp(`^${labels[kind]}:`)}).click();
    await waitOptions(ctx, ui.capture, kind);
    ctx.assert(await ui.session.page.locator('[data-usage-state="committed"]').count() === 0, 'options_waited_for_report');
    const menu = ui.session.page.getByRole('menu', {name: `${labels[kind]} choices`, exact: true});
    first = await menu.locator('[data-id]').evaluateAll(nodes => nodes.map(n => n.getAttribute('data-id')).filter(Boolean));
    ctx.assert(first.length === 50, 'choices_pagination_fixture');
  });
  await ctx.step('step_2', async () => {
    const menu = ui.session.page.getByRole('menu', {name: `${labels[kind]} choices`, exact: true});
    const next = ui.session.page.waitForResponse(r => canonicalPath(r.url()).endsWith('/metrics/options') && r.request().postDataJSON()?.cursor);
    await menu.getByRole('button', {name: 'Next choices', exact: true}).click();
    const second = (await (await next).json()).items;
    ctx.assert(second.length > 0 && second.every(x => !first.includes(x.value)), 'choice_page_duplicate');
    await menu.getByRole('button', {name: 'Previous choices', exact: true}).click();
    await ui.session.page.getByLabel(`Search ${labels[kind].toLowerCase()} choices`, {exact: true}).fill(ui.f.choices[kind].search);
    await waitOptions(ctx, ui.capture, kind, ui.f.choices[kind].search);
    const result = ui.capture.rows.filter(r => r.request_body?.search === ui.f.choices[kind].search).at(-1);
    ctx.assert(equal(result.body.items.map(x => x.value).sort(), [...ui.f.choices[kind].search_ids].sort()), 'search_choice_cohort');
    const target = menu.locator(`[data-id=${JSON.stringify(ui.f.choices[kind].selected.id)}]`);
    ctx.assert((await target.innerText()).includes(ui.f.choices[kind].selected.label), 'choice_readable_name'); await target.click();
  });
  await ctx.step('step_3', async () => {
    await choose(ctx, ui.session, 'owner', ui.f.owner_change.first);
    await choose(ctx, ui.session, 'sandbox', ui.f.owner_change.sandbox);
    await choose(ctx, ui.session, 'owner', ui.f.owner_change.second);
    ctx.assert(await ui.session.page.getByRole('button', {name: 'Sandbox: All Sandboxes', exact: true}).isVisible(), 'owner_change_retained_sandbox');
    await hold.release(); await switchReadAM(ctx, ui.session);
    const report = await apply(ctx, ui);
    ctx.assert(report.query.filters.owner === ui.f.owner_change.second.id && !report.query.filters.sandbox_id && !report.query.filters.agent_id, 'dependent_scope_not_cleared');
  });
  await ctx.step('expected_result', async () => {
    await ui.capture.flush(); const rows = ui.capture.rows.filter(r => r.path.includes('/metrics/options'));
    ctx.assert(rows.length > 0 && rows.every(r => r.method === 'POST' && !r.path.includes('?')
      && r.request_body.limit <= 50 && r.request_id), 'option_search_url_or_bound');
    ctx.assert(rows.some(r => r.request_body.kind === 'sandbox' && r.request_body.owner === ui.f.owner_change.first.id), 'sandbox_choices_not_owner_scoped');
  });
}));
export const cases = [...choices, define(19, ['browser', 'admin-fixtures'], async ctx => {
  const ui = await setup(ctx), f = ui.f; let report, sort;
  await ctx.step('step_1', async () => {
    await ready(ctx, ui); await choose(ctx, ui.session, 'owner', f.filter_selection.owner_choice);
    await choose(ctx, ui.session, 'sandbox', f.filter_selection.sandbox_choice);
    await choose(ctx, ui.session, 'runtime', f.filter_selection.runtime_choice);
    await pick(ui.session.page, 'Usage Sandbox status', f.filter_selection.status.replaceAll('_', ' '));
    report = await apply(ctx, ui); assertSelection(ctx, report, f.filter_selection);
    const params = new URL(ui.session.page.url()).searchParams;
    for (const [key, value] of Object.entries({owner: f.filter_selection.owner, sandbox: f.filter_selection.sandbox_id,
      runtime: f.filter_selection.runtime_version, status: f.filter_selection.status})) ctx.assert(params.get(key) === value, 'filter_url_scope');
  });
  await ctx.step('step_2', async () => {
    await switchReadAM(ctx, ui.session); await ui.session.page.reload();
    assertSelection(ctx, await ready(ctx, ui), f.filter_selection);
    await pick(ui.session.page, 'Sort usage inventory', 'Owner'); sort = new URL(ui.session.page.url()).searchParams.get('sort');
    await ui.session.page.getByRole('button', {name: 'Clear filters', exact: true}).click();
    report = await ready(ctx, ui);
    ctx.assert(!report.query.filters.owner && !report.query.filters.sandbox_id && !report.query.filters.runtime_version
      && report.query.filters.statuses.length === 0, 'clear_filters_report_scope');
    ctx.assert(new URL(ui.session.page.url()).searchParams.get('sort') === sort, 'clear_changed_sort');
  });
  await ctx.step('step_3', async () => {
    await ui.session.page.goto(new URL('/operations?search=' + encodeURIComponent(f.legacy_search), ui.session.origin).href);
    await visible(ctx, ui.session.page.getByText('This saved report has a legacy search. Choose a Sandbox and apply filters to replace it.', {exact: true}));
    await choose(ctx, ui.session, 'sandbox', f.filter_selection.sandbox_choice); report = await apply(ctx, ui);
    ctx.assert(report.query.filters.search === '' && report.query.filters.sandbox_id === f.filter_selection.sandbox_id, 'legacy_search_not_replaced');
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(!new URL(ui.session.page.url()).searchParams.has('search') && report.scope.principal === f.principal, 'legacy_scope_or_principal');
  });
})];
