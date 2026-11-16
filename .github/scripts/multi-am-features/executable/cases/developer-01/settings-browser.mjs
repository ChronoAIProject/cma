import {define, setup, browser, evidence, settingsSession, navigateSettings, saveSettings,
  invalidSettings, request, read, roleRequest, P, other, equal, sameSettings, required} from './common.mjs';

const retention = page => page.getByLabel('Agent log retention (days)', {exact: true});
const save = page => page.getByRole('button', {name: 'Save settings', exact: true});
const changedDays = value => value === 30 ? 31 : 30;

export const retentionCase = define(44, 'browser', 2, async ctx => {
  const f = await setup(ctx, 'settings_retention');
  const state = await settingsSession(ctx, f), session = await browser(ctx);
  await navigateSettings(session);
  const days = changedDays(state.original.settings.retention.agent_log_days);
  let changed;
  await ctx.step('step_1', async () => {
    const desired = structuredClone(state.original.settings);
    desired.retention.agent_log_days = days; state.own(desired);
    await retention(session.page).fill(String(days));
    await saveSettings(session);
    changed = await read(ctx, P + '/settings', other(ctx.am));
    ctx.assert(changed.revision > state.original.revision
      && changed.settings.retention.agent_log_days === days, 'admin_retention_save');
    for (const input of ['-1', '1.5', '4294967296', '']) {
      await retention(session.page).fill(input);
      ctx.assert(!(await retention(session.page).evaluate(node => node.checkValidity())),
        'admin_retention_invalid_form');
      await save(session.page).click();
      ctx.assert(equal(changed, await read(ctx, P + '/settings')), 'admin_retention_invalid_ui_saved');
    }
    for (const agent_log_days of [-1, 1.5, 4294967296, 'not-a-number']) {
      await invalidSettings(ctx, {retention: {...changed.settings.retention, agent_log_days}});
    }
  });
  await ctx.step('step_2', async () => {
    ctx.assert(equal(changed, await read(ctx, P + '/settings', other(ctx.am))), 'admin_retention_peer');
    await state.restore();
    ctx.assert(sameSettings((await read(ctx, P + '/settings')).settings, state.original.settings),
      'admin_retention_restored');
  });
  await ctx.step('expected_result', async () => {
    const expected = {...state.original.settings, retention: changed.settings.retention};
    ctx.assert(sameSettings(changed.settings, expected), 'admin_retention_erased_other_settings');
    await evidence(ctx, session);
  });
}, ['admin-fixtures', 'browser']);

export const settingsConflict = define(41, 'browser', 3, async ctx => {
  const f = await setup(ctx, 'settings_conflict');
  required(typeof f.faults?.withholdNextSuccessfulResponse === 'function', 'admin_settings_fault_missing');
  const state = await settingsSession(ctx, f);
  const first = await browser(ctx), second = await browser(ctx, 'admin', other(ctx.am));
  let before, winner, draftDays;
  await ctx.step('step_1', async () => {
    await navigateSettings(first); await navigateSettings(second);
    before = await read(ctx, P + '/settings');
    ctx.assert(equal(before, await read(ctx, P + '/settings', other(ctx.am))), 'admin_settings_initial_revision');
    for (const session of [first, second]) {
      ctx.assert(await session.page.getByText('Revision ' + before.revision, {exact: true}).isVisible(),
        'admin_settings_browser_revision');
    }
  });
  await ctx.step('step_2', async () => {
    const days = changedDays(before.settings.retention.agent_log_days);
    const desired = structuredClone(before.settings); desired.retention.agent_log_days = days; state.own(desired);
    draftDays = days + 1;
    await retention(second.page).fill(String(draftDays));
    await retention(first.page).fill(String(days)); await saveSettings(first);
    winner = await read(ctx, P + '/settings');
    await save(second.page).click();
    await second.page.getByText(/Your draft is retained/).waitFor();
    ctx.assert(await retention(second.page).inputValue() === String(draftDays), 'admin_conflict_draft_lost');
    ctx.assert(await save(second.page).isDisabled(), 'admin_conflict_replay_enabled');
    ctx.assert(equal(winner, await read(ctx, P + '/settings', other(ctx.am))), 'admin_stale_draft_wrote');
  });
  await ctx.step('step_3', async () => {
    await second.page.getByRole('button', {name: 'Review retained draft', exact: true}).click();
    ctx.assert(await retention(second.page).inputValue() === String(draftDays), 'admin_review_draft_lost');
    const desired = structuredClone(winner.settings); desired.retention.agent_log_days = draftDays; state.own(desired);
    const fault = await f.faults.withholdNextSuccessfulResponse({method: 'PUT', path: P + '/settings'});
    required(typeof fault.cleanup === 'function', 'admin_fault_cleanup_missing');
    ctx.defer('admin_settings_fault_cleanup', () => fault.cleanup());
    await save(second.page).click();
    await second.page.getByText(/Your draft is retained/).waitFor();
    ctx.assert(await save(second.page).isDisabled(), 'admin_uncertain_replay_enabled');
    const current = await read(ctx, P + '/settings');
    ctx.assert(current.settings.retention.agent_log_days === draftDays, 'admin_unknown_write_not_applied');
    const receipt = await ctx.observe('admin_withheld_response', () => fault.receipt());
    ctx.assert(receipt.forwarded_writes === 1 && receipt.following_requests?.[0]?.method === 'GET',
      'admin_uncertain_write_replayed');
    await second.page.getByRole('button', {name: 'Load current values', exact: true}).click();
    ctx.assert(await retention(second.page).inputValue() === String(draftDays), 'admin_load_current_value');
    required(typeof f.concurrentLists === 'function', 'admin_concurrent_list_setup_missing');
    const lists = await f.concurrentLists(ctx);
    required(lists.initial && lists.server && lists.draft && typeof lists.cleanup === 'function',
      'admin_concurrent_list_data_missing');
    ctx.defer('admin_concurrent_list_cleanup', () => lists.cleanup());
    // Data provides actual valid policy values; the test performs both competing writes.
    const latest = await read(ctx, P + '/settings'); state.own({...latest.settings, model_budget: lists.initial});
    await request(ctx, 'PATCH', P + '/settings', {expected_revision: latest.revision, model_budget: lists.initial});
    await navigateSettings(second);
    const field = second.page.getByLabel('Owner override 1: owner ID', {exact: true});
    await field.fill(lists.draft.owners[0].owner);
    const live = await read(ctx, P + '/settings'); state.own({...live.settings, model_budget: lists.server});
    await request(ctx, 'PATCH', P + '/settings', {expected_revision: live.revision, model_budget: lists.server});
    await save(second.page).click();
    await second.page.getByText(/Owner overrides or request models changed both here and on the server/).waitFor();
    ctx.assert(await second.page.getByRole('button', {name: 'Review retained draft', exact: true}).isDisabled(),
      'admin_unsafe_list_merge_allowed');
  });
  await ctx.step('expected_result', async () => {
    await evidence(ctx, first); await evidence(ctx, second); await state.restore();
    ctx.assert(sameSettings((await read(ctx, P + '/settings')).settings, state.original.settings),
      'admin_conflict_restoration');
  });
}, ['admin-fixtures', 'browser']);

export const settingsRead = define(40, 'browser', 2, async ctx => {
  const f = await setup(ctx, 'settings_read');
  required(typeof f.faults?.failNextRead === 'function', 'admin_read_fault_missing');
  const state = await settingsSession(ctx, f), session = await browser(ctx, 'settings_operator');
  let draft;
  await ctx.step('step_1', async () => {
    await navigateSettings(session);
    const current = await read(ctx, P + '/settings');
    ctx.assert(await session.page.getByText('Revision ' + current.revision, {exact: true}).isVisible(),
      'admin_settings_read_revision');
    for (const [label, value] of [
      ['Agent log retention (days)', current.settings.retention.agent_log_days],
      ['Sandboxes per user', current.settings.limits.max_sandboxes_per_user],
      ['Visitor Sandboxes', current.settings.limits.visitor_max_sandboxes],
      ['Visitor launches per day', current.settings.limits.visitor_max_launches_per_day],
    ]) ctx.assert(await session.page.getByLabel(label, {exact: true}).inputValue() === String(value),
      'admin_settings_read_value');
    required(current.model_budget_writes_available !== true, 'admin_unavailable_writer_setup_missing');
    ctx.assert(await session.page.getByLabel('Enable spend controls', {exact: true}).isDisabled(),
      'admin_unavailable_spend_editable');
    await roleRequest(ctx, f, 'read_operator', 'GET', P + '/settings', undefined, {allowed: [403]});
  });
  await ctx.step('step_2', async () => {
    draft = String(changedDays(state.original.settings.retention.agent_log_days));
    await retention(session.page).fill(draft);
    const fault = await f.faults.failNextRead({path: P + '/settings'});
    ctx.defer('admin_settings_read_fault_cleanup', () => fault.cleanup());
    await session.page.getByRole('button', {name: 'Refresh settings availability', exact: true}).click();
    await session.page.getByText(/The current settings could not be read/).waitFor();
    ctx.assert(await retention(session.page).inputValue() === draft, 'admin_refresh_error_lost_draft');
    await session.page.getByRole('button', {name: 'Read current settings', exact: true}).click();
    await session.page.getByText(/The current revision is /).waitFor();
    ctx.assert(await retention(session.page).inputValue() === draft, 'admin_refresh_retry_lost_draft');
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(await save(session.page).isDisabled(), 'admin_read_uncertainty_allows_save');
    ctx.assert(equal(await read(ctx, P + '/settings'), state.original), 'admin_read_changed_settings');
    await evidence(ctx, session);
  });
}, ['admin-fixtures', 'browser']);
