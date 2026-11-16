import {define, setup, settingsSession, browser, evidence, navigateSettings, saveSettings,
  required, read, request, P, other, equal, sameSettings} from './common.mjs';
import {fillFields, ownerFields} from './spend-browser.mjs';

export const ownerOverrides = define(56, 'browser', 2, async ctx => {
  const f = await setup(ctx, 'owner_overrides'), state = await settingsSession(ctx, f);
  required(f.ownerId && f.policy && f.editedPolicy && f.serverPolicy && f.maxOwners?.length === 1024
    && typeof f.admitModelRequest === 'function', 'admin_owner_override_fixture_missing');
  const s = await browser(ctx); let expected;
  const save = () => s.page.getByRole('button', {name: 'Save settings', exact: true});
  await ctx.step('step_1', async () => {
    const initial = await read(ctx, P + '/settings');
    required(initial.settings.model_budget?.owners.length === 0, 'admin_empty_owner_policy_fixture_required');
    await navigateSettings(s);
    await s.page.getByRole('button', {name: 'Add owner override', exact: true}).click();
    await s.page.getByLabel('Owner override 1: owner ID', {exact: true}).fill(f.ownerId);
    await fillFields(s.page, ownerFields, f.policy, 'Owner override 1: ');
    expected = {...initial.settings, model_budget: {...initial.settings.model_budget,
      owners: [{owner: f.ownerId, policy: f.policy}]}}; state.own(expected); await saveSettings(s);
    const reservation = await ctx.observe('admin_owner_override_admission', () => f.admitModelRequest(ctx));
    ctx.assert(equal(reservation.policy, f.policy), 'admin_override_not_applied');
    await fillFields(s.page, ownerFields, f.editedPolicy, 'Owner override 1: ');
    const current = await read(ctx, P + '/settings');
    const concurrent = {...current.settings.model_budget, owners: [{owner: f.ownerId, policy: f.serverPolicy}]};
    state.own({...current.settings, model_budget: concurrent});
    await request(ctx, 'PATCH', P + '/settings', {expected_revision: current.revision, model_budget: concurrent}, {am: other(ctx.am)});
    await save().click();
    await s.page.getByText(/Owner overrides or request models changed both here and on the server/).waitFor();
    ctx.assert(await s.page.getByRole('button', {name: 'Review retained draft', exact: true}).isDisabled(),
      'admin_override_unsafe_merge');
    await s.page.getByRole('button', {name: 'Load current values', exact: true}).click();
    await fillFields(s.page, ownerFields, f.editedPolicy, 'Owner override 1: ');
    expected = {...current.settings, model_budget: {...concurrent, owners: [{owner: f.ownerId, policy: f.editedPolicy}]}};
    state.own(expected); await saveSettings(s);
    await s.page.getByRole('button', {name: 'Add owner override', exact: true}).click();
    await s.page.getByLabel('Owner override 2: owner ID', {exact: true}).fill(f.ownerId);
    await fillFields(s.page, ownerFields, f.policy, 'Owner override 2: ');
    await save().click();
    ctx.assert(await s.page.getByLabel('Owner override 2: owner ID', {exact: true}).getAttribute('aria-invalid') === 'true',
      'admin_duplicate_owner_not_rejected');
    ctx.assert(equal((await read(ctx, P + '/settings')).settings.model_budget, expected.model_budget),
      'admin_duplicate_owner_saved');
    await s.page.getByRole('button', {name: 'Remove owner override 2', exact: true}).click();
    await s.page.getByRole('button', {name: 'Remove owner override 1', exact: true}).click();
    expected = {...expected, model_budget: {...expected.model_budget, owners: []}}; state.own(expected); await saveSettings(s);
    ctx.assert((await read(ctx, P + '/settings')).settings.model_budget.owners.length === 0, 'admin_owner_remove_failed');
    const beforeMax = await read(ctx, P + '/settings');
    const maxBudget = {...beforeMax.settings.model_budget, owners: f.maxOwners};
    await state.patch('model_budget', maxBudget); await navigateSettings(s);
    ctx.assert(await s.page.getByRole('button', {name: 'Add owner override', exact: true}).isDisabled(),
      'admin_owner_maximum_not_enforced');
  });
  await ctx.step('step_2', async () => {
    const peer = await read(ctx, P + '/settings', other(ctx.am));
    ctx.assert(equal(peer.settings.model_budget.owners, f.maxOwners), 'admin_override_peer_list');
    await state.restore();
    ctx.assert(sameSettings((await read(ctx, P + '/settings')).settings, state.original.settings), 'admin_override_restore');
  });
  await ctx.step('expected_result', async () => { await evidence(ctx, s); });
}, ['admin-fixtures', 'browser']);

export const requestModels = define(57, 'browser', 2, async ctx => {
  const f = await setup(ctx, 'request_models'), state = await settingsSession(ctx, f);
  required(f.model && f.unvaluedModel && Number.isSafeInteger(f.overhead)
    && typeof f.setRequestWritesAvailable === 'function' && typeof f.admitModelRequest === 'function',
  'admin_request_model_fixture_missing');
  ctx.defer('admin_request_writer_restore', () => f.setRequestWritesAvailable(true));
  const s = await browser(ctx); let desired;
  await ctx.step('step_1', async () => {
    const initial = await read(ctx, P + '/settings');
    required(initial.settings.model_budget?.request_models?.length === 0, 'admin_empty_model_fixture_required');
    await navigateSettings(s);
    await s.page.getByRole('button', {name: 'Add request model', exact: true}).click();
    await s.page.getByLabel('Request model 1: model name', {exact: true}).fill(f.model);
    await s.page.getByLabel('Request model 1: input overhead (tokens)', {exact: true}).fill(String(f.overhead));
    desired = {...initial.settings, model_budget: {...initial.settings.model_budget,
      request_models: [{model: f.model, input_overhead_tokens: f.overhead}]}};
    state.own(desired); await saveSettings(s);
    ctx.assert(equal((await read(ctx, P + '/settings')).settings.model_budget.request_models,
      desired.model_budget.request_models), 'admin_request_model_saved_values');
    await s.page.getByRole('button', {name: 'Add request model', exact: true}).click();
    await s.page.getByLabel('Request model 2: model name', {exact: true}).fill(f.model);
    await s.page.getByLabel('Request model 2: input overhead (tokens)', {exact: true}).fill('1');
    await s.page.getByRole('button', {name: 'Save settings', exact: true}).click();
    ctx.assert(await s.page.getByLabel('Request model 2: model name', {exact: true}).getAttribute('aria-invalid') === 'true',
      'admin_request_model_duplicate_allowed');
    await s.page.getByRole('button', {name: 'Remove request model 2', exact: true}).click();
    await s.page.getByLabel('Request model 1: model name', {exact: true}).fill(f.unvaluedModel);
    await s.page.getByRole('button', {name: 'Save settings', exact: true}).click();
    ctx.assert(await s.page.locator('[aria-invalid="true"]').count() > 0, 'admin_request_model_valuation_not_checked');
    ctx.assert(equal((await read(ctx, P + '/settings')).settings.model_budget, desired.model_budget),
      'admin_invalid_model_saved');
    await navigateSettings(s);
    const accepted = await ctx.observe('admin_listed_model_admission', () => f.admitModelRequest(ctx, f.model));
    const denied = await ctx.observe('admin_unlisted_model_admission', () => f.admitModelRequest(ctx, f.unlistedModel));
    ctx.assert(accepted.status >= 200 && accepted.status < 300 && accepted.reservation?.model === f.model,
      'admin_listed_model_not_admitted');
    ctx.assert(denied.status >= 400 && denied.reservation === null && denied.provider_requests === 0,
      'admin_unlisted_model_reached_provider');
    await s.page.getByRole('button', {name: 'Remove request model 1', exact: true}).click();
    const removed = {...desired, model_budget: {...desired.model_budget, request_models: []}};
    state.own(removed); await saveSettings(s);
    ctx.assert((await read(ctx, P + '/settings')).settings.model_budget.request_models.length === 0,
      'admin_model_removal_failed');
    await state.patch('model_budget', desired.model_budget);
    await f.setRequestWritesAvailable(false); await navigateSettings(s);
    ctx.assert(await s.page.getByLabel('Request model 1: model name', {exact: true}).inputValue() === f.model
      && await s.page.getByLabel('Request model 1: model name', {exact: true}).isDisabled(),
    'admin_request_unavailable_lost_or_enabled_values');
    ctx.assert(await s.page.getByRole('button', {name: 'Save settings', exact: true}).isDisabled(),
      'admin_request_unavailable_writer_can_save');
  });
  await ctx.step('step_2', async () => {
    ctx.assert(equal((await read(ctx, P + '/settings', other(ctx.am))).settings.model_budget, desired.model_budget),
      'admin_request_model_peer');
    await f.setRequestWritesAvailable(true); await state.restore();
    ctx.assert(sameSettings((await read(ctx, P + '/settings')).settings, state.original.settings),
      'admin_request_model_restore');
  });
  await ctx.step('expected_result', async () => { await evidence(ctx, s); });
}, ['admin-fixtures', 'browser']);
