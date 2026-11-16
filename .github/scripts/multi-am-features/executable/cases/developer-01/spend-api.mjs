import {define, setup, required, request, roleRequest, read, P, other, equal} from './common.mjs';

export const spendRead = define(77, 'global', 2, async ctx => {
  const f = await setup(ctx, 'spend_read');
  required(typeof f.selectMode === 'function' && f.expectedEnabled && f.expectedDisabled,
    'admin_spend_modes_missing');
  await ctx.step('step_1', async () => {
    for (const [mode, expected] of [['enabled', f.expectedEnabled], ['disabled', f.expectedDisabled]]) {
      await f.selectMode(mode);
      for (const am of ['a', 'b']) {
        const current = await read(ctx, P + '/spend', am);
        ctx.assert(current.metering_enabled === (mode === 'enabled')
          && current.settings_revision === expected.settings_revision, 'admin_spend_mode_revision');
        for (const field of ['levels', 'committed', 'ledger']) {
          ctx.assert(equal(current[field], expected[field]), 'admin_spend_accounting_mismatch');
        }
      }
    }
  });
  await ctx.step('step_2', async () => {
    await f.selectMode('dependency_unavailable');
    for (const am of ['a', 'b']) {
      const result = await request(ctx, 'GET', P + '/spend', undefined, {am, allowed: [503]});
      ctx.assert(result.body?.code === 'platform_spend_unavailable'
        && !Object.hasOwn(result.body, 'committed'), 'admin_spend_unavailable_reported_zero');
    }
  });
  await ctx.step('expected_result', async () => {
    await f.selectMode('enabled_without_cloud');
    const current = await read(ctx, P + '/spend');
    ctx.assert(current.metering_enabled === true && current.ledger
      && !Object.hasOwn(current.ledger, 'cloud'), 'admin_missing_cloud_fabricated_zero');
    await roleRequest(ctx, f, 'user', 'GET', P + '/spend', undefined, {allowed: [403]});
    ctx.assert(equal(current, await read(ctx, P + '/spend', other(ctx.am))), 'admin_spend_peer_mismatch');
  });
});

export const spendObservation = define(78, 'global', 2, async ctx => {
  const f = await setup(ctx, 'spend_observation');
  required(typeof f.setWritesAvailable === 'function' && Number.isSafeInteger(f.units) && f.units > 0
    && Number.isFinite(Date.parse(f.observedThrough)) && Date.parse(f.observedThrough) <= Date.now(),
  'admin_cloud_observation_fixture_missing');
  let submitted;
  await ctx.step('step_1', async () => {
    const initial = await read(ctx, P + '/spend');
    ctx.assert(initial.metering_enabled === true, 'admin_cloud_metering_disabled');
    const body = {source: 'cloud', units: f.units, observed_through: f.observedThrough};
    submitted = (await request(ctx, 'POST', P + '/spend/observations', body)).body;
    const current = await read(ctx, P + '/spend', other(ctx.am));
    ctx.assert(current.ledger?.cloud?.units === f.units
      && Date.parse(current.ledger.cloud.observed_through) === Date.parse(f.observedThrough)
      && equal(current.ledger.cloud, submitted.ledger?.cloud), 'admin_cloud_observation_not_persisted');
  });
  await ctx.step('step_2', async () => {
    for (const body of [
      {source: 'cloud', units: Number.MAX_SAFE_INTEGER + 1, observed_through: f.observedThrough},
      {source: 'cloud', units: f.units, observed_through: new Date(Date.now() + 86400000).toISOString()},
      {source: 'invalid', units: f.units, observed_through: f.observedThrough},
    ]) {
      await request(ctx, 'POST', P + '/spend/observations', body, {allowed: [422]});
      ctx.assert(equal((await read(ctx, P + '/spend')).ledger.cloud, submitted.ledger.cloud),
        'admin_invalid_cloud_observation_committed');
    }
    await f.setWritesAvailable(false);
    const refused = await request(ctx, 'POST', P + '/spend/observations', {
      source: 'cloud', units: f.units + 1, observed_through: f.observedThrough,
    }, {allowed: [503]});
    ctx.assert(refused.body?.code === 'model_budget_writes_unavailable', 'admin_cloud_wrong_writer_refusal');
  });
  await ctx.step('expected_result', async () => {
    await roleRequest(ctx, f, 'user', 'POST', P + '/spend/observations', {
      source: 'cloud', units: f.units + 1, observed_through: f.observedThrough,
    }, {allowed: [403]});
    ctx.assert(equal((await read(ctx, P + '/spend', other(ctx.am))).ledger.cloud, submitted.ledger.cloud),
      'admin_cloud_refusal_changed_ledger');
  });
});
