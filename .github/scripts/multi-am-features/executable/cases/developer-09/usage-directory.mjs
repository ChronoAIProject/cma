import {define, equal, visible, id, pick} from './helpers.mjs';
import {setup, ready} from './usage-ui.mjs';
export const cases = [define(28, ['browser', 'admin-fixtures'], async ctx => {
  const runs = [];
  await ctx.step('step_1', async () => {
    for (const actor of ['names_operator', 'directory_operator']) {
      const ui = await setup(ctx, actor), report = await ready(ctx, ui);
      const result = await ctx.request({actor, method: 'POST', path: '/api/v1/admin/usage/metrics/options',
        body: {kind: 'owner', search: ui.f.directory.duplicate_label, limit: 50}});
      const duplicates = result.body.items.filter(r => ui.f.directory.duplicate_ids.includes(r.value));
      ctx.assert(duplicates.length === 2 && duplicates.every(r => r.label === ui.f.directory.duplicate_label)
        && new Set(duplicates.map(r => r.disambiguator)).size === 2, 'owner_names_not_disambiguated');
      await ui.session.page.getByRole('button', {name: /^Owner:/}).click();
      await ui.session.page.getByLabel('Search owner choices', {exact: true}).fill(ui.f.directory.duplicate_label);
      const menu = ui.session.page.getByRole('menu', {name: 'Owner choices', exact: true});
      for (const option of duplicates) {
        const item = menu.locator(`[data-id=${JSON.stringify(option.value)}]`); await visible(ctx, item);
        ctx.assert((await item.innerText()).includes(option.label) && (await item.innerText()).includes(option.disambiguator), 'directory_context_not_rendered');
      }
      await ui.session.page.keyboard.press('Escape');
      await pick(ui.session.page, 'Allocation chart grouping', 'By owner');
      await visible(ctx, ui.session.page.getByRole('list', {name: 'Owner allocation values', exact: true}));
      const allocations = await ctx.request({actor, method: 'GET', path: `/api/v1/admin/usage/metrics/reports/${id(report.report_id)}/owner-allocations?metric=cpu&limit=200`});
      ctx.assert(allocations.body.report_id === report.report_id, 'directory_report_identity');
      runs.push({actor, ui, duplicates, allocations: allocations.body});
    }
  });
  await ctx.step('step_2', async () => {
    for (const run of runs) {
      const {ui, actor} = run;
      const text = JSON.stringify(run.duplicates) + JSON.stringify(run.allocations);
      const contexts = ui.f.directory.private_context;
      ctx.assert(contexts.length > 0 && (actor === 'names_operator'
        ? contexts.every(value => !text.includes(value)) : contexts.every(value => text.includes(value))), 'directory_precision_capability');
      await ui.session.page.getByRole('button', {name: `Runtime details for ${ui.f.directory.sandbox_name}`, exact: true}).click();
      const detail = ui.session.page.getByRole('dialog', {name: 'Sandbox details', exact: true});
      await visible(ctx, detail); const runtimeText = await detail.innerText();
      ctx.assert(runtimeText.includes(ui.f.directory.duplicate_label), 'public_runtime_owner_name');
      if (actor === 'names_operator') ctx.assert(contexts.every(value => !runtimeText.includes(value)), 'runtime_directory_context_leak');
      await detail.getByRole('button', {name: 'Close', exact: true}).click();
      await ui.session.page.getByRole('tab', {name: 'Activity', exact: true}).click();
      const activity = ui.session.page.getByRole('table', {name: 'Activity', exact: true}); await visible(ctx, activity);
      ctx.assert((await activity.innerText()).includes(ui.f.directory.duplicate_label), 'public_activity_owner_name');
      run.activityText = await activity.innerText();
    }
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(equal(runs[0].duplicates.map(r => r.value).sort(), runs[1].duplicates.map(r => r.value).sort()), 'directory_capability_changed_identity_cohort');
    ctx.assert(runs[0].ui.f.directory.private_context.every(value => !runs[0].activityText.includes(value)), 'activity_directory_context');
    const restore = await runs[1].ui.f.revokeDirectory('directory_operator'); ctx.defer('restore_directory_capability', restore);
    const revoked = await ctx.request({actor: 'directory_operator', am: ctx.am === 'a' ? 'b' : 'a', method: 'POST',
      path: '/api/v1/admin/usage/metrics/options', body: {kind: 'owner', search: runs[1].ui.f.directory.duplicate_label, limit: 50}});
    ctx.assert(runs[1].ui.f.directory.private_context.every(value => !JSON.stringify(revoked.body).includes(value)), 'revoked_directory_context_cached');
  });
})];
