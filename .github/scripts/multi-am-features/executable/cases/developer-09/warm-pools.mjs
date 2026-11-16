import {canonicalPath, define, take, open, goto, visible, other, id, equal, poll} from './helpers.mjs';
const P = '/api/v1/admin/warm-pools';
const list = (ctx, am = other(ctx.am)) => ctx.request({actor: 'admin', am, method: 'GET', path: P});
const row = (session, pool, previous = false) => session.page.getByRole('table', {name: previous ? 'Previous runtime pools' : 'Current runtime pools', exact: true})
  .getByRole('row', {name: `Warm pool ${pool.cpu_millis} millicores / ${pool.memory_mib} MiB`, exact: true});
async function setup(ctx) {
  const f = await take(ctx, 'warm_pools'), session = await open(ctx);
  await goto(session, '/operations?tab=warm_pools');
  await session.page.getByRole('heading', {name: 'Warm pools', exact: true}).waitFor({state: 'visible'});
  return {f, session};
}
async function converged(ctx, f, poolId, target) {
  const result = await poll(ctx, 'pool_capacity', () => list(ctx), r => r.body.items.some(p => p.id === poolId
    && p.target_replicas === target && p.observed && p.ready_replicas === target));
  const native = await f.observeProvider();
  const pool = native.pools.find(p => p.id === poolId);
  ctx.assert(pool?.spare_ids.length === target && pool.spare_ids.every(i => !native.claimed_ids.includes(i)), 'pool_spare_observer');
  return result.body.items.find(p => p.id === poolId);
}
export function sameIntent(receipts) {
  return receipts.length === 2 && receipts.every(r => r.method === 'POST' && r.status === 202)
    && /^[a-f0-9]{64}$/.test(receipts[0].idempotency_sha256 ?? '')
    && receipts[0].idempotency_sha256 === receipts[1].idempotency_sha256
    && equal(receipts[0].request_body, receipts[1].request_body)
    && receipts[0].body?.id === receipts[1].body?.id;
}
export const cases = [define(60, ['browser', 'admin-fixtures'], async ctx => {
  const {f, session} = await setup(ctx); let pools;
  await ctx.step('step_1', async () => {
    pools = (await list(ctx)).body;
    ctx.assert(pools.default_image === f.current_image && pools.default_cpu_millis === f.default_cpu_millis
      && pools.default_memory_mib === f.default_memory_mib, 'warm_pool_defaults');
    ctx.assert((await session.page.locator('.warm-pool-image').first().innerText()).includes(f.current_image), 'warm_image_rendered');
    for (const pool of pools.items) {
      const item = row(session, pool, pool.image !== pools.default_image); await visible(ctx, item);
      const cells = await item.getByRole('cell').allTextContents();
      ctx.assert(cells[2].trim() === String(pool.target_replicas), 'desired_capacity_rendered');
      ctx.assert(cells[3].trim() === (pool.observed ? String(pool.ready_replicas) : 'Unknown'), 'ready_capacity_rendered');
      ctx.assert(cells[4].trim() === (pool.observed ? String(Math.max(0, pool.target_replicas - pool.ready_replicas)) : 'Unknown'), 'pending_capacity_rendered');
    }
    ctx.assert(pools.items.some(p => p.id === f.unknown_pool_id && !p.observed)
      && pools.items.some(p => p.id === f.unavailable_pool_id && p.status === 'unavailable'), 'unknown_unavailable_pool_fixture');
  });
  await ctx.step('step_2', async () => {
    await session.switchAM(other(ctx.am));
    const manual = session.page.waitForResponse(r => canonicalPath(r.url()) === P && r.request().method() === 'GET');
    await session.page.getByRole('button', {name: 'Refresh pools', exact: true}).click();
    ctx.assert((await manual).status() === 200, 'manual_pool_refresh');
    const started = performance.now();
    const automatic = await session.page.waitForResponse(r => canonicalPath(r.url()) === P && r.request().method() === 'GET', {timeout: 15000});
    ctx.assert(automatic.status() === 200 && performance.now() - started <= 15000, 'visible_ten_second_refresh');
    const pool = pools.items.find(p => p.id === f.current_pool_id);
    await row(session, pool).getByText('Pool configuration', {exact: true}).click();
    ctx.assert((await row(session, pool).innerText()).includes(f.configuration_hash), 'pool_configuration_identity');
    ctx.assert(pools.items.some(p => p.id === f.previous_pool_id && p.image === f.previous_image), 'previous_pool_fixture');
  });
  await ctx.step('expected_result', async () => {
    const provider = await ctx.observe('pool_native_capacity', () => f.observeProvider());
    for (const pool of pools.items.filter(p => p.observed)) {
      const actual = provider.pools.find(x => x.id === pool.id);
      ctx.assert(actual?.spare_ids.length === pool.ready_replicas
        && actual.spare_ids.every(i => !provider.claimed_ids.includes(i)), 'claimed_counted_as_spare');
    }
  });
}), define(61, ['browser', 'admin-fixtures'], async ctx => {
  const {f, session} = await setup(ctx), faults = await take(ctx, 'usage_faults'); let drop, pool;
  const form = session.page.locator('.warm-pool-create').filter({hasText: 'Configure a warm pool'});
  await ctx.step('step_1', async () => {
    await form.getByText('Configure a warm pool', {exact: true}).click();
    await form.getByLabel('CPU (millicores)', {exact: true}).fill(String(f.cpu_millis));
    await form.getByLabel('Memory (MiB)', {exact: true}).fill(String(f.memory_mib));
    await form.getByLabel('Ready Sandbox target', {exact: true}).fill('1001');
    ctx.assert(await form.getByRole('button', {name: 'Save pool target', exact: true}).isDisabled(), 'pool_bounds_not_enforced');
    await form.getByLabel('Ready Sandbox target', {exact: true}).fill('1');
    ctx.assert(await form.getByRole('button', {name: 'Save pool target', exact: true}).isEnabled(), 'pool_valid_target_disabled');
  });
  await ctx.step('step_2', async () => {
    drop = await faults.arm({kind: 'drop_accepted_response', method: 'POST', path: P}); ctx.defer('pool_drop', () => drop.close());
    await form.getByRole('button', {name: 'Save pool target', exact: true}).click();
    await visible(ctx, form.getByText('The result is unconfirmed. Retry keeps the original resources and request identity.', {exact: true}));
    ctx.assert(await form.getByLabel('CPU (millicores)', {exact: true}).isDisabled(), 'uncertain_resources_editable');
    await drop.release(); await session.switchAM(other(ctx.am));
    await form.getByRole('button', {name: 'Retry same request', exact: true}).click();
    await visible(ctx, form.getByText('Pool target saved. Ready capacity updates after the controller applies it.', {exact: true}));
    const receipts = (await drop.observations()).filter(r => r.method === 'POST');
    ctx.assert(sameIntent(receipts), 'warm_pool_rekey_or_body_change');
    ctx.assert(receipts[0].body.status === 'pending' || receipts[0].body.observed === false, 'pool_acceptance_pending_not_observed'); pool = receipts[1].body;
  });
  await ctx.step('step_3', async () => {
    await converged(ctx, f, pool.id, 1);
    await row(session, pool).getByRole('button', {name: 'Disable pool', exact: true}).click();
    await converged(ctx, f, pool.id, 0); ctx.assert(true, 'pool_disabled');
  });
  await ctx.step('expected_result', async () => {
    const pools = (await list(ctx)).body.items.filter(p => p.cpu_millis === f.cpu_millis && p.memory_mib === f.memory_mib && p.image === f.current_image);
    ctx.assert(pools.length === 1 && pools[0].id === pool.id && pools[0].target_replicas === 0, 'idempotent_pool_count');
  });
}), define(62, ['browser', 'admin-fixtures'], async ctx => {
  const {f, session} = await setup(ctx), faults = await take(ctx, 'usage_faults'); let original, item, updated, held;
  await ctx.step('step_1', async () => {
    original = (await list(ctx)).body.items.find(p => p.id === f.current_pool_id);
    ctx.assert(original.target_replicas === 0, 'resize_zero_fixture'); item = row(session, original); await visible(ctx, item);
    held = await faults.arm({kind: 'hold_response', method: 'GET', path: P}); ctx.defer('pool_stale_read_hold', () => held.close());
    const updatedResponse = await ctx.request({actor: 'admin', am: other(ctx.am), method: 'PUT', path: P + '/' + id(original.id),
      body: {expected_revision: original.revision, target_replicas: 1}, allowed: [202]}); updated = updatedResponse.body;
    // The open row retains its read revision; submit that intent before its next refresh.
    await item.getByLabel('Ready Sandbox target', {exact: true}).fill('1');
    const conflict = session.page.waitForResponse(r => canonicalPath(r.url()) === P + '/' + id(original.id) && r.request().method() === 'PUT');
    await item.getByRole('button', {name: 'Update target', exact: true}).click();
    ctx.assert((await conflict).status() === 409, 'stale_pool_update_accepted');
    ctx.assert(await item.getByLabel('Ready Sandbox target', {exact: true}).inputValue() === '1'
      && await item.getByRole('button', {name: 'Update target', exact: true}).isDisabled(), 'pool_draft_not_retained_or_replay_enabled');
  });
  await ctx.step('step_2', async () => {
    await held.release();
    const response = session.page.waitForResponse(r => canonicalPath(r.url()) === P && r.request().method() === 'GET');
    await session.page.getByRole('button', {name: 'Read current target', exact: true}).click();
    const readback = await (await response).json();
    ctx.assert(readback.items.find(p => p.id === original.id)?.revision === updated.revision, 'pool_readback_revision');
    await item.getByLabel('Ready Sandbox target', {exact: true}).fill('0');
    await item.getByRole('button', {name: 'Update target', exact: true}).click();
    await converged(ctx, f, original.id, 0); ctx.assert(true, 'explicit_pool_change_complete');
  });
  await ctx.step('expected_result', async () => {
    const current = (await list(ctx)).body.items.find(p => p.id === original.id);
    ctx.assert(current.revision === updated.revision + 1 && current.target_replicas === 0, 'pool_stale_effect_or_revision');
  });
}), define(63, ['browser', 'admin-fixtures'], async ctx => {
  const {f, session} = await setup(ctx); let pools;
  await ctx.step('step_1', async () => {
    pools = (await list(ctx)).body.items; const pool = pools.find(p => p.id === f.current_pool_id);
    ctx.assert(pool.target_replicas === 1, 'disable_owned_spare_fixture');
    await row(session, pool).getByRole('button', {name: 'Disable pool', exact: true}).click();
    await converged(ctx, f, pool.id, 0);
  });
  await ctx.step('step_2', async () => {
    const pool = pools.find(p => p.id === f.previous_pool_id), item = row(session, pool, true);
    await visible(ctx, item);
    ctx.assert(await item.getByLabel('Ready Sandbox target', {exact: true}).count() === 0
      && await item.getByRole('button', {name: 'Update target', exact: true}).count() === 0, 'previous_image_resize_enabled');
    await item.getByRole('button', {name: 'Disable pool', exact: true}).click();
    await converged(ctx, f, pool.id, 0);
  });
  await ctx.step('step_3', async () => {
    const witness = await ctx.observe('claimed_agent_continuity', () => f.claimedAgentWitness());
    ctx.assert(witness.agent_id === f.claimed_agent_id && witness.serving === true && witness.same_runtime === true, 'claimed_agent_interrupted');
  });
  await ctx.step('expected_result', async () => {
    const current = (await list(ctx)).body.items;
    ctx.assert([f.current_pool_id, f.previous_pool_id].every(i => current.some(p => p.id === i && p.target_replicas === 0 && p.ready_replicas === 0)), 'disabled_capacity_not_final');
  });
})];
