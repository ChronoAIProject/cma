import {setup, settingsSession, browser, navigateSettings, read, P, other, apiPath,
  equal, sameSettings, required, check} from './common.mjs';
import {fillFields, ownerFields} from './spend-browser.mjs';
import {operationsPrefetch} from './operations-prefetch.mjs';

const SETTINGS = P + '/settings';
const REPORTS = P + '/usage/metrics/reports';
const READS = new Set(['GET', 'HEAD', 'OPTIONS']);

function ownerAdmission(result, fixture, owner, policy, code) {
  check(result?.status >= 200 && result.status < 300 && result.outcome === 'accepted'
    && result.owner === owner && result.model === fixture.model && result.scope === fixture.scope
    && result.surface === fixture.admissionSurface && result.request_id?.trim(), code + '_identity');
  const reservation = result.reservation;
  check(reservation && reservation.id?.trim() && reservation.owner === owner
    && reservation.model === fixture.model && reservation.scope === fixture.scope
    && reservation.provider === fixture.provider && equal(reservation.policy, policy), code + '_reservation');
  const provider = result.provider_request;
  check(provider && provider.id?.trim() && provider.reservation_id === reservation.id
    && provider.owner === owner && provider.model === fixture.model
    && provider.scope === fixture.scope && provider.provider === fixture.provider, code + '_provider');
}

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
  return {status: response.status(), request_id: await response.headerValue('x-request-id'), body,
    expected_revision: expectedRevision, request_settings: structuredClone(settings)};
}

export async function defaultOwnerPolicyScenario(ctx) {
  const f = await setup(ctx, 'default_owner_policy'), state = await settingsSession(ctx, f);
  required(f.policy && f.overridePolicy && typeof f.readReservations === 'function'
    && typeof f.admitModelRequest === 'function' && typeof f.scope === 'string'
    && typeof f.provider === 'string' && typeof f.admissionSurface === 'string',
  'admin_default_budget_fixture_missing');
  required(typeof f.ownerWithoutOverride === 'string' && typeof f.ownerWithOverride === 'string'
    && f.ownerWithoutOverride.trim() && f.ownerWithOverride.trim()
    && f.ownerWithoutOverride !== f.ownerWithOverride && typeof f.model === 'string' && f.model.trim(),
  'admin_default_budget_owners_missing');
  const session = await browser(ctx), reports = operationsPrefetch(ctx, session);
  const mutations = [], pending = new Set(), captureFailures = [], byRequest = new WeakMap();
  const onRequest = request => {
    try {
      if (READS.has(request.method()) || apiPath(request.url()) === REPORTS) return;
      check(mutations.length < 256, 'admin_default_policy_request_bound');
      const row = {method: request.method(), path: apiPath(request.url()), body: request.postDataJSON()};
      mutations.push(row); byRequest.set(request, row);
    } catch (error) {captureFailures.push(error);}
  };
  const onResponse = response => {
    const row = byRequest.get(response.request()); if (!row) return;
    const task = (async () => {
      row.response = {status: response.status(), request_id: await response.headerValue('x-request-id'),
        body: await response.json()};
    })();
    pending.add(task); void task.then(() => pending.delete(task), error => {
      captureFailures.push(error); pending.delete(task);
    });
  };
  session.page.on('request', onRequest); session.page.on('response', onResponse);
  let reservations, initial, saved, finalityJob;
  const saveReceipts = [];
  const finalize = () => finalityJob ??= (async () => {
    const errors = [];
    try {await session.close();} catch (error) {errors.push(error);}
    session.page.off('request', onRequest); session.page.off('response', onResponse);
    try {
      while (pending.size) await Promise.allSettled([...pending]);
      const proof = await reports.capture('admin_default_policy_browser_final', true);
      check(proof.authentication === 'browser_session', 'admin_default_policy_cookie_session');
      const writes = proof.requests.filter(row => !READS.has(row.method));
      check(writes.length === mutations.length && writes.length === saveReceipts.length && writes.length === 2,
        'admin_default_policy_write_count');
      check(captureFailures.length === 0, captureFailures[0] ?? 'admin_default_policy_capture_failed');
      for (const [index, receipt] of saveReceipts.entries()) {
        const row = mutations[index], observed = writes.filter(value => value.request_id === receipt.request_id);
        check(row?.method === 'PUT' && row.path === SETTINGS && equal(row.body,
          {expected_revision: receipt.expected_revision, settings: receipt.request_settings}),
        'admin_default_policy_save_body_capture');
        check(row.response?.request_id === receipt.request_id && row.response.status === receipt.status
          && observed.length === 1 && observed[0].method === 'PUT' && observed[0].status === receipt.status
          && observed[0].am === ctx.am && observed[0].serving_am === ctx.am,
        'admin_default_policy_ingress');
      }
    } catch (error) {errors.push(error);}
    try {
      const retained = await ctx.observe('admin_reservations_after', () => f.readReservations(ctx));
      check(Array.isArray(retained.items) && reservations.items.every(before =>
        retained.items.some(after => equal(after, before))), 'admin_default_policy_reservations_erased');
    } catch (error) {errors.push(error);}
    try {await state.restore();} catch (error) {errors.push(error);}
    if (errors.length) throw new AggregateError(errors,
      'admin_default_policy_finality ' + errors.map(error => error?.message ?? String(error)).join(' '));
  })();
  ctx.defer('admin_default_policy_finalize', finalize);
  await ctx.step('step_1', async () => {
    reservations = await ctx.observe('admin_reservations_before', () => f.readReservations(ctx));
    required(Array.isArray(reservations.items) && reservations.items.length > 0,
      'admin_existing_reservations_missing');
    initial = await read(ctx, SETTINGS);
    required(initial.settings.model_budget?.default_policy === undefined,
      'admin_default_policy_absent_fixture_required');
    required(initial.settings.model_budget.enabled === true && initial.settings.model_budget.platform
      && initial.settings.model_budget.platform.cloud,
      'admin_default_policy_enabled_fixture_required');
    const configuredOwners = initial.settings.model_budget.owners ?? [];
    required(configuredOwners.some(row => row.owner === f.ownerWithOverride
      && equal(row.policy, f.overridePolicy)) && !configuredOwners.some(row => row.owner === f.ownerWithoutOverride),
    'admin_default_policy_owner_binding');
    required((initial.settings.model_budget.request_models ?? []).some(row => row.model === f.model),
      'admin_default_policy_model_binding');
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
    ownerAdmission(applied, f, f.ownerWithoutOverride, f.policy, 'admin_default_policy_scope');
    const override = await ctx.observe('admin_override_policy_admission', () => f.admitModelRequest(ctx, {
      owner: f.ownerWithOverride, model: f.model ?? undefined,
    }));
    ownerAdmission(override, f, f.ownerWithOverride, f.overridePolicy, 'admin_default_policy_override_scope');
    check(applied.provider_requests === 1 && override.provider_requests === 1,
      'admin_default_policy_provider_effect');
    check(!equal(applied.reservation, override.reservation), 'admin_default_policy_distinct_reservations');
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
    check(noDefault?.status >= 400 && noDefault.owner === f.ownerWithoutOverride
      && noDefault.model === f.model && noDefault.scope === f.scope
      && noDefault.surface === f.admissionSurface && noDefault.request_id?.trim()
      && noDefault.outcome === 'refused' && noDefault.reservation === null
      && noDefault.provider_request === null && noDefault.provider_requests === 0
      && noDefault.error?.type === 'BudgetError' && noDefault.error.code === 'budget_unavailable'
      && noDefault.error.reason === 'unbudgeted', 'admin_default_policy_removed_scope');
  });
  await ctx.step('step_2', async () => {
    await finalize();
    for (const am of [ctx.am, other(ctx.am)]) ctx.assert(sameSettings((await read(ctx, SETTINGS, am)).settings,
      state.original.settings), 'admin_default_policy_restore');
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(saved.body.settings.model_budget.default_policy === undefined,
      'admin_default_policy_removed');
  });
}
