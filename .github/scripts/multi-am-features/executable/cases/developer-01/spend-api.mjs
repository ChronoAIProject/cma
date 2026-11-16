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

export {spendObservation} from './cloud-observation.mjs';
