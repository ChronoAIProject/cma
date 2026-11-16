import {define, setup, request, P, other, equal, check, required} from './common.mjs';
import {validateObservationSpend, durableSpend, settingsControl, requireObservation,
  acceptedObservation, invalidCloudObservations} from './cloud-observation-data.mjs';

export const spendObservation = define(78, 'global', 2, async ctx => {
  const f = await setup(ctx, 'spend_observation');
  required(f.exclusiveSettings === true && typeof f.setWritesAvailable === 'function',
    'admin_cloud_observation_fixture_missing');
  let original, initial, accepted, writeIntent = false, modeIntent = false, finalityJob, finalized = false;
  const response = async (label, am, method, path, body, options = {}) => {
    const value = await request(ctx, method, path, body, {am, ...options});
    await ctx.observe(label, async () => ({am, response: value})); return value;
  };
  const settings = async (am, label) => {
    const value = (await response(label, am, 'GET', P + '/settings')).body;
    check(Number.isSafeInteger(value?.revision) && value.revision >= 0 && value.settings
      && typeof value.platform_spend_writes_available === 'boolean', 'admin_cloud_settings_shape');
    return value;
  };
  const spend = async (am, label) => {
    const value = (await response(label, am, 'GET', P + '/spend')).body;
    check(validateObservationSpend(value), 'admin_cloud_spend_shape'); return value;
  };
  const unchanged = value => check(equal(durableSpend(value), durableSpend(accepted ?? initial)), 'admin_cloud_durable_changed');
  const unchangedSettings = (value, available) => {
    check(value.platform_spend_writes_available === available, 'admin_cloud_writer_gate');
    check(equal(settingsControl(value), settingsControl(original)), 'admin_cloud_settings_changed');
  };
  const both = async label => {
    for (const am of [ctx.am, other(ctx.am)]) unchanged(await spend(am, label));
  };
  const finalize = () => finalityJob ??= (async () => {
    if (!original || !initial) return;
    const errors = [];
    if (writeIntent && !accepted) errors.push(Error('admin_cloud_observation_outcome_unproven'));
    // The parent owns the real gate. One restoration follows even an unknown
    // disable acknowledgement; an unknown restoration never authorizes replay.
    if (modeIntent) try {await f.setWritesAvailable(original.platform_spend_writes_available);} catch (e) {errors.push(e);}
    for (const am of ['a', 'b']) {
      try {unchangedSettings(await settings(am, 'admin_cloud_final_settings'), original.platform_spend_writes_available);}
      catch (e) {errors.push(e);}
      try {
        const current = await spend(am, 'admin_cloud_final_spend');
        if (!writeIntent || accepted) unchanged(current);
      } catch (e) {errors.push(e);}
    }
    if (errors.length) throw new AggregateError(errors, 'admin_cloud_finality');
    finalized = true;
  })();
  ctx.defer('admin_cloud_finalize', finalize);
  const post = async (am, body, label, status = 200, actor = 'admin') => {
    await ctx.observe('admin_cloud_intent', async () => ({am, actor, method: 'POST', path: P + '/spend/observations', body, label}));
    if (status === 200) writeIntent = true;
    return response(label, am, 'POST', P + '/spend/observations', body, {allowed: [status], actor});
  };
  const refused = async (am, body, status, code, label, actor = 'admin') => {
    const result = await post(am, body, label, status, actor);
    check(result.status === status && result.body?.code === code, 'admin_cloud_wrong_refusal');
    check(['ledger', 'committed', 'levels', 'units'].every(k => !Object.hasOwn(result.body, k)),
      'admin_cloud_refusal_exposed_accounting');
    await both('admin_cloud_after_refusal');
  };
  let body, different;
  await ctx.step('step_1', async () => {
    const firstSettings = await settings(ctx.am, 'admin_cloud_initial_settings');
    const secondSettings = await settings(other(ctx.am), 'admin_cloud_initial_settings');
    const first = await spend(ctx.am, 'admin_cloud_initial_spend');
    const second = await spend(other(ctx.am), 'admin_cloud_initial_spend');
    ctx.assert(equal(firstSettings, secondSettings) && equal(durableSpend(first), durableSpend(second)),
      'admin_cloud_initial_peer_mismatch');
    requireObservation(f, firstSettings, first);
    original = structuredClone(firstSettings); initial = structuredClone(first);
    body = {source: 'cloud', units: f.units, observed_through: f.observedThrough};
    different = {...body, units: f.units === Number.MAX_SAFE_INTEGER ? f.units - 1 : f.units + 1};
    accepted = acceptedObservation(initial, original, body, (await post(ctx.am, body, 'admin_cloud_accepted')).body);
    await both('admin_cloud_after_save');
    ctx.assert(accepted.ledger.cloud.units === f.units, 'admin_cloud_units_not_saved');
  });
  await ctx.step('step_2', async () => {
    for (const invalid of invalidCloudObservations(body, original.settings.model_budget.platform.period_start)) {
      for (const am of ['a', 'b']) await refused(am, invalid, 422, 'invalid_request', 'admin_cloud_invalid');
    }
    modeIntent = true; await f.setWritesAvailable(false);
    for (const am of ['a', 'b']) unchangedSettings(await settings(am, 'admin_cloud_unavailable_settings'), false);
    await both('admin_cloud_before_unavailable');
    for (const am of ['a', 'b']) await refused(am, different, 503, 'model_budget_writes_unavailable', 'admin_cloud_unavailable');
    ctx.assert(modeIntent, 'admin_cloud_unavailable_not_exercised');
  });
  await ctx.step('expected_result', async () => {
    for (const am of ['a', 'b']) await refused(am, different, 403, 'forbidden', 'admin_cloud_unauthorized', 'user');
    await finalize(); ctx.assert(finalized, 'admin_cloud_not_finalized');
  });
});
