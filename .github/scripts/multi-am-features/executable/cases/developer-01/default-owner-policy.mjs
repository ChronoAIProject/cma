import {setup, settingsSession, browser, evidence, navigateSettings, read, P, other, apiPath,
  equal, sameSettings, required, check} from './common.mjs';
import {fillFields, ownerFields} from './spend-browser.mjs';

const SETTINGS = P + '/settings';

async function savePolicy(session, expectedRevision, settings, status = 200) {
  const [response] = await Promise.all([
    session.page.waitForResponse(r => r.request().method() === 'PUT' && apiPath(r.url()) === SETTINGS),
    session.page.getByRole('button', {name: 'Save settings', exact: true}).click(),
  ]);
  const body = await response.json();
  check(response.status() === status && (await response.headerValue('x-request-id'))?.trim(),
    'admin_default_policy_save_response');
  check(equal(response.request().postDataJSON(), {expected_revision: expectedRevision, settings}),
    'admin_default_policy_save_body');
  if (status === 200) check(Number.isSafeInteger(body.revision) && body.revision > expectedRevision
    && equal(body.settings, settings), 'admin_default_policy_saved_policy');
  return {status: response.status(), request_id: await response.headerValue('x-request-id'), body};
}

export async function defaultOwnerPolicyScenario(ctx) {
  const f = await setup(ctx, 'default_owner_policy'), state = await settingsSession(ctx, f);
  required(f.policy && f.overridePolicy && typeof f.readReservations === 'function'
    && typeof f.admitModelRequest === 'function', 'admin_default_budget_fixture_missing');
  required(typeof f.ownerWithoutOverride === 'string' && typeof f.ownerWithOverride === 'string'
    && f.ownerWithoutOverride !== f.ownerWithOverride, 'admin_default_budget_owners_missing');
  const session = await browser(ctx); let reservations, initial, saved, finalityJob;
  const saveReceipts = [];
  const finalize = () => finalityJob ??= (async () => {
    const errors = [];
    try {await session.close();} catch (error) {errors.push(error);}
    try {
      const proof = await evidence(ctx, session);
      check(proof.authentication === 'browser_session', 'admin_default_policy_cookie_session');
      const writes = proof.requests.filter(row => row.method === 'PUT');
      check(writes.length === saveReceipts.length && writes.length === 2,
        'admin_default_policy_write_count');
      for (const [index, receipt] of saveReceipts.entries()) {
        const row = writes[index];
        check(row.request_id === receipt.request_id && row.am === ctx.am && row.serving_am === ctx.am,
          'admin_default_policy_ingress');
      }
    } catch (error) {errors.push(error);}
    try {await state.restore();} catch (error) {errors.push(error);}
    if (errors.length) throw new AggregateError(errors, 'admin_default_policy_finality');
  })();
  ctx.defer('admin_default_policy_finalize', finalize);
  await ctx.step('step_1', async () => {
    reservations = await ctx.observe('admin_reservations_before', () => f.readReservations(ctx));
    required(Array.isArray(reservations.items) && reservations.items.length > 0,
      'admin_existing_reservations_missing');
    initial = await read(ctx, SETTINGS);
    required(initial.settings.model_budget?.default_policy === undefined,
      'admin_default_policy_absent_fixture_required');
    required(initial.settings.model_budget.enabled === true && initial.settings.model_budget.platform,
      'admin_default_policy_enabled_fixture_required');
    await navigateSettings(session);
    const configure = session.page.getByRole('button', {name: 'Configure default owner policy', exact: true});
    if (await configure.isVisible()) await configure.click();
    await fillFields(session.page, ownerFields, f.policy);
    const desired = {...initial.settings, model_budget: {...initial.settings.model_budget, default_policy: f.policy}};
    state.own(desired);
    saved = await savePolicy(session, initial.revision, desired); saveReceipts.push(saved);
    const peer = await read(ctx, SETTINGS, other(ctx.am));
    ctx.assert(peer.revision === saved.body.revision && equal(peer.settings, desired),
      'admin_default_policy_peer_readback');
    const applied = await ctx.observe('admin_default_policy_admission', () => f.admitModelRequest(ctx, {
      owner: f.ownerWithoutOverride, model: f.model ?? undefined,
    }));
    check(applied.status >= 200 && applied.status < 300 && applied.owner === f.ownerWithoutOverride
      && equal(applied.policy, f.policy), 'admin_default_policy_scope');
    const override = await ctx.observe('admin_override_policy_admission', () => f.admitModelRequest(ctx, {
      owner: f.ownerWithOverride, model: f.model ?? undefined,
    }));
    check(override.status >= 200 && override.status < 300 && override.owner === f.ownerWithOverride
      && equal(override.policy, f.overridePolicy) && !equal(override.policy, f.policy),
    'admin_default_policy_override_scope');
    check(applied.provider_requests === 1 && override.provider_requests === 1,
      'admin_default_policy_provider_effect');
    await session.page.getByRole('button', {name: 'Remove default owner policy', exact: true}).click();
    const afterRemoval = await read(ctx, SETTINGS);
    const removed = {...afterRemoval.settings, model_budget: {...afterRemoval.settings.model_budget,
      default_policy: undefined}};
    delete removed.model_budget.default_policy;
    state.own(removed);
    saved = await savePolicy(session, afterRemoval.revision, removed); saveReceipts.push(saved);
    const noDefault = await ctx.observe('admin_default_policy_removed_admission', () => f.admitModelRequest(ctx, {
      owner: f.ownerWithoutOverride, model: f.model ?? undefined,
    }));
    check(noDefault.status >= 400 && noDefault.reservation === null && noDefault.provider_requests === 0,
      'admin_default_policy_removed_scope');
  });
  await ctx.step('step_2', async () => {
    const retained = await ctx.observe('admin_reservations_after', () => f.readReservations(ctx));
    check(Array.isArray(retained.items) && reservations.items.every(before =>
      retained.items.some(after => equal(after, before))), 'admin_default_policy_reservations_erased');
    await finalize();
    for (const am of [ctx.am, other(ctx.am)]) ctx.assert(sameSettings((await read(ctx, SETTINGS, am)).settings,
      state.original.settings), 'admin_default_policy_restore');
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(saved.body.settings.model_budget.default_policy === undefined,
      'admin_default_policy_removed');
  });
}
