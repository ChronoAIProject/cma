import {define, take, open, goto, visible, other, id} from './helpers.mjs';
const tabs = [['sandboxes', 'Sandboxes & usage'], ['warm_pools', 'Warm pools'],
  ['settings', 'Platform settings'], ['activity', 'Activity']];
async function selected(ctx, page, label) {
  const tab = page.getByRole('tab', {name: label, exact: true});
  await visible(ctx, tab);
  ctx.assert(await tab.getAttribute('aria-selected') === 'true', 'selected_tab_mismatch');
  ctx.assert(await page.getByRole('tabpanel', {name: label, exact: true}).isVisible(), 'panel_mismatch');
}
export const cases = [define(1, ['browser'], async ctx => {
  let session;
  await ctx.step('step_1', async () => {
    session = await open(ctx); await goto(session, '/');
    await visible(ctx, session.page.getByRole('button', {name: /^Account menu for /}));
  });
  await ctx.step('step_2', async () => {
    const {page} = session;
    await page.getByRole('button', {name: /^Account menu for /}).click();
    await page.getByRole('menuitem', {name: 'Operations', exact: true}).click();
    await visible(ctx, page.getByRole('heading', {name: 'Operations', exact: true}));
    await goto(session, '/'); await page.keyboard.press('ControlOrMeta+k');
    await page.getByRole('combobox', {name: 'Search commands'}).fill('Operations');
    await page.getByRole('option', {name: /Operations/}).click();
    ctx.assert(new URL(page.url()).pathname === '/operations', 'command_navigation_route');
    await goto(session, '/operations'); await selected(ctx, page, tabs[0][1]);
  });
  await ctx.step('step_3', async () => {
    ctx.assert(JSON.stringify(await session.page.getByRole('tab').allTextContents())
      === JSON.stringify(tabs.map(x => x[1])), 'operator_tabs_incomplete');
    ctx.assert(new URL(session.page.url()).pathname === '/operations', 'operations_route');
  });
  await ctx.step('expected_result', async () => {
    const user = await open(ctx, 'user'); await goto(user, '/operations');
    await visible(ctx, user.page.getByRole('heading', {name: 'Operations unavailable', exact: true}));
    ctx.assert(await user.page.getByRole('tablist', {name: 'Operations views'}).count() === 0, 'privileged_tabs_disclosed');
    ctx.assert(await user.page.locator('[aria-label="Operations resource dashboard"]').count() === 0, 'privileged_report_disclosed');
  });
}), define(2, ['browser', 'admin-fixtures'], async ctx => {
  await take(ctx, 'admin_roles'); let session;
  await ctx.step('step_1', async () => {
    session = await open(ctx);
    for (const [key, label] of tabs) {
      await goto(session, '/operations?tab=' + key); await selected(ctx, session.page, label);
      await session.page.reload(); await selected(ctx, session.page, label);
    }
  });
  await ctx.step('step_2', async () => {
    await goto(session, '/operations?tab=usage'); await selected(ctx, session.page, tabs[0][1]);
    ctx.assert(new URL(session.page.url()).searchParams.get('tab') === null, 'legacy_tab_not_migrated');
  });
  await ctx.step('step_3', async () => {
    const {page} = session;
    await page.getByRole('tab', {name: tabs[0][1], exact: true}).focus();
    for (const [key, label] of [['ArrowRight', tabs[1][1]], ['End', tabs[3][1]],
      ['ArrowRight', tabs[0][1]], ['ArrowLeft', tabs[3][1]], ['Home', tabs[0][1]]]) {
      await page.keyboard.press(key); await selected(ctx, page, label);
      ctx.assert(await page.getByRole('tab', {name: label, exact: true}).evaluate(e => e === document.activeElement), 'tab_focus');
    }
    await page.getByRole('tab', {name: 'Activity', exact: true}).click();
    await page.goBack(); await selected(ctx, page, tabs[0][1]);
    await page.goForward(); await selected(ctx, page, 'Activity');
    const limited = await open(ctx, 'read_operator');
    for (const key of ['settings', 'unknown', 'warm_pools']) {
      await goto(limited, '/operations?tab=' + key); await selected(ctx, limited.page, tabs[0][1]);
      ctx.assert(await limited.page.getByRole('tab').count() === 1, 'limited_operator_tabs');
    }
  });
  await ctx.step('expected_result', async () => {
    await selected(ctx, session.page, 'Activity');
    ctx.assert(new URL(session.page.url()).searchParams.get('tab') === 'activity', 'navigation_url_mismatch');
  });
}), define(3, ['browser', 'admin-fixtures'], async ctx => {
  const f = await take(ctx, 'admin_roles'); let reader;
  const permissions = [
    ['read_operator', ['Sandboxes & usage'], '/sandboxes'],
    ['control_operator', [], null], ['settings_operator', ['Warm pools', 'Platform settings'], '/settings'],
    ['activity_operator', ['Activity'], '/activity'],
  ];
  await ctx.step('step_1', async () => {
    for (const [actor, expected, permitted] of permissions) {
      const session = await open(ctx, actor); if (actor === 'read_operator') reader = session;
      await goto(session, '/operations');
      if (expected.length) await selected(ctx, session.page, expected[0]);
      else await visible(ctx, session.page.getByRole('heading', {name: 'Operations unavailable'}));
      ctx.assert(JSON.stringify(await session.page.getByRole('tab').allTextContents()) === JSON.stringify(expected), 'capability_tab_boundary');
      for (const path of ['/sandboxes', '/settings', '/activity']) {
        const response = await ctx.request({actor, method: 'GET', path: '/api/v1/admin' + path,
          allowed: [path === permitted ? 200 : 403]});
        ctx.assert(response.status === (path === permitted ? 200 : 403), 'capability_endpoint_boundary');
      }
    }
  });
  await ctx.step('step_2', async () => {
    const reportPath = '/api/v1/admin/usage/metrics/reports/' + id(f.report_id);
    const prior = await ctx.request({actor: 'read_operator', am: ctx.am, method: 'GET', path: reportPath, allowed: [200]});
    ctx.assert(prior.body.report_id === f.report_id && prior.body.scope?.admin === true, 'prior_report_access');
    ctx.defer('restore_read_capability', await f.revoke('read_operator', 'read_all_sandboxes'));
    await reader.switchAM(other(ctx.am)); await reader.page.reload();
    await visible(ctx, reader.page.getByRole('heading', {name: 'Operations unavailable'}));
    for (const path of ['/sandboxes/' + id(f.sandbox_id), '/usage/metrics/reports/' + id(f.report_id)]) {
      const denied = await ctx.request({actor: 'read_operator', am: other(ctx.am), method: 'GET', path: '/api/v1/admin' + path, allowed: [403, 404]});
      ctx.assert(!JSON.stringify(denied.body).includes(f.sandbox_id), 'revoked_record_disclosed');
    }
  });
  await ctx.step('step_3', async () => {
    const denied = await ctx.request({actor: 'activity_operator', method: 'POST',
      path: '/api/v1/admin/sandboxes/' + id(f.sandbox_id) + '/always-on',
      body: {always_on: true, expected_revision: 0}, allowed: [403]});
    const content = JSON.stringify(denied.body) + await reader.page.locator('body').innerText();
    ctx.assert(f.private_markers.length > 0 && f.private_markers.every(m => !content.includes(m)), 'denied_private_state');
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(await reader.page.locator('[aria-label="Operations resource dashboard"]').count() === 0, 'revoked_report_retained');
    ctx.assert(await reader.page.getByRole('tablist', {name: 'Operations views'}).count() === 0, 'revoked_tabs_retained');
  });
})];
