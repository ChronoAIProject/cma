import {define, setup, browser, evidence, required, P, equal} from './common.mjs';
import {activityPages} from './activity.mjs';

export const activityTable = define(70, 'browser', 2, async ctx => {
  const f = await setup(ctx, 'activity_table');
  required(f.records?.length >= 51 && f.actor && f.actorLabel && f.action && f.actionLabel,
    'admin_activity_browser_cohort_missing');
  const s = await browser(ctx, 'activity_operator');
  let rows;
  const table = s.page.getByRole('table', {name: 'Activity', exact: true});
  await ctx.step('step_1', async () => {
    await s.page.goto(new URL('/operations?tab=activity', s.origin).href);
    await table.waitFor();
    for (const [label, value] of [['Activity actor', f.actorLabel], ['Activity action', f.actionLabel]]) {
      await s.page.getByRole('button', {name: label, exact: true}).click();
      await s.page.getByRole('menuitemradio', {name: value, exact: true}).click();
    }
    await s.page.getByRole('button', {name: 'Apply filters', exact: true}).click();
    const firstMarker = f.records[0].target.id, nextMarker = f.records[50].target.id;
    await table.getByText(firstMarker, {exact: true}).waitFor();
    const initial = await table.innerText();
    await s.page.getByRole('button', {name: 'Refresh records', exact: true}).click();
    await table.getByText(firstMarker, {exact: true}).waitFor();
    ctx.assert(await table.innerText() === initial, 'admin_activity_refresh_changed_cohort');
    await s.switchAM(ctx.fixture('browser').otherAM(ctx.am));
    await s.page.getByRole('button', {name: 'Next page', exact: true}).click();
    await table.getByText(nextMarker, {exact: true}).waitFor();
    ctx.assert(!(await table.getByText(firstMarker, {exact: true}).isVisible()), 'admin_activity_page_overlap');
    await s.page.getByRole('button', {name: 'Previous page', exact: true}).click();
    await table.getByText(firstMarker, {exact: true}).waitFor();
    ctx.assert(await table.innerText() === initial, 'admin_activity_previous_page_mismatch');
    rows = await activityPages(ctx, {actor: f.actor, action: f.action}, {summary: true});
    ctx.assert(equal(rows.map(r => r.id), f.records.map(r => r.id)), 'admin_activity_frozen_cohort_mismatch');
  });
  await ctx.step('step_2', async () => {
    for (const [status, label] of [['succeeded', 'Succeeded'], ['refused', 'Refused'], ['failed', 'Failed']]) {
      const record = f.records.slice(0, 50).find(r => r.outcome.status === status);
      required(record, 'admin_activity_outcome_fixture_missing');
      const row = table.getByRole('row').filter({hasText: record.target.id});
      ctx.assert(await row.getByText(label, {exact: true}).isVisible(), 'admin_activity_outcome_label');
      const time = row.locator('time');
      ctx.assert(await time.getAttribute('datetime') === record.at, 'admin_activity_timestamp_identity');
      const expected = new Date(record.at).toLocaleString('en-SG', {
        timeZone: 'Asia/Singapore', dateStyle: 'medium', timeStyle: 'short',
      });
      ctx.assert(await time.textContent() === expected, 'admin_activity_timestamp_not_sgt');
      ctx.assert(await row.getByText(record.actor_name?.trim() || 'Name unavailable', {exact: true}).isVisible(),
        'admin_activity_actor_label');
    }
  });
  await ctx.step('expected_result', async () => {
    const receipt = await evidence(ctx, s);
    ctx.assert(receipt.requests.some(r => r.serving_am === 'a')
      && receipt.requests.some(r => r.serving_am === 'b'),
    'admin_activity_missing_ingress');
  });
}, ['admin-fixtures', 'browser']);
