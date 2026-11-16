import {define, setup, required, roleRequest, id, equal, other} from './common.mjs';
import {access, capabilities} from './access-common.mjs';
import {poll} from './enforcement.mjs';

export async function sandboxRead(ctx, f, role, am = ctx.am) {
  return (await roleRequest(ctx, f, role, 'GET', '/api/v1/sandboxes/' + id(f.sandboxId), undefined,
    {am, agentId: f.agentId})).body;
}
export async function command(ctx, f, role, kind, key, revision) {
  const current = revision === undefined ? await sandboxRead(ctx, f, role) : null;
  const body = {command: kind, expected_revision: revision ?? current.revision};
  const response = await roleRequest(ctx, f, role, 'POST', '/api/v1/sandboxes/' + id(f.sandboxId) + '/commands',
    body, {agentId: f.agentId, key, allowed: [202]});
  ctx.assert(response.body.command === kind && response.body.sandbox_id === f.sandboxId
    && response.body.expected_revision === body.expected_revision, 'admin_sandbox_command_identity');
  return {receipt: response.body, body, key};
}
export async function settle(ctx, f, role, intent, expected = 'applied') {
  const final = await poll(async () => (await roleRequest(ctx, f, role, 'GET',
    '/api/v1/sandbox-commands/' + id(intent.receipt.command_id), undefined,
    {am: other(ctx.am), agentId: f.agentId})).body,
  value => value.state.status !== 'queued', 'admin_sandbox_command_timeout');
  ctx.assert(final.command_id === intent.receipt.command_id && final.state.status === expected,
    'admin_sandbox_command_disposition');
  return final;
}

export const sandboxCommands = define(83, 'agent', 3, async ctx => {
  const f = await setup(ctx, 'sandbox_commands');
  required(f.sandboxId && f.agentId && f.keys && typeof f.holdCommand === 'function'
    && typeof f.revokeControl === 'function', 'admin_sandbox_command_fixture_missing');
  let initial;
  await ctx.step('step_1', async () => {
    capabilities(ctx, await access(ctx, f, 'control_operator', 'sandbox', f.sandboxId), ['manage_sandbox']);
    initial = await sandboxRead(ctx, f, 'control_operator');
    ctx.assert(initial.status === 'active', 'admin_command_initial_active');
  });
  await ctx.step('step_2', async () => {
    for (const [kind, expected] of [['pause', 'paused'], ['revive', 'active']]) {
      const intent = await command(ctx, f, 'control_operator', kind, f.keys[kind]);
      const final = await settle(ctx, f, 'control_operator', intent);
      const replay = await roleRequest(ctx, f, 'control_operator', 'POST',
        '/api/v1/sandboxes/' + id(f.sandboxId) + '/commands', intent.body,
        {agentId: f.agentId, am: other(ctx.am), key: intent.key, allowed: [202]});
      ctx.assert(replay.body.command_id === final.command_id, 'admin_sandbox_command_replay_duplicate');
      for (const am of ['a', 'b']) {
        const observed = await poll(() => sandboxRead(ctx, f, 'creator', am),
          s => s.status === expected, 'admin_sandbox_lifecycle_timeout');
        ctx.assert(observed.id === f.sandboxId, 'admin_sandbox_lifecycle_identity');
      }
    }
  });
  await ctx.step('step_3', async () => {
    const stale = await roleRequest(ctx, f, 'control_operator', 'POST',
      '/api/v1/sandboxes/' + id(f.sandboxId) + '/commands',
      {command: 'pause', expected_revision: initial.revision},
      {agentId: f.agentId, key: f.keys.stale, allowed: [409]});
    ctx.assert(stale.status === 409, 'admin_sandbox_stale_admitted');
    for (const mode of ['queue_expired', 'authority_revoked']) {
      const barrier = await f.holdCommand(); ctx.defer('admin_sandbox_barrier_cleanup', () => barrier.cleanup());
      const intent = await command(ctx, f, 'control_operator', 'pause', f.keys[mode]);
      ctx.assert(intent.receipt.state.status === 'queued', 'admin_sandbox_barrier_not_queued');
      if (mode === 'queue_expired') {
        const elapsed = await ctx.observe('admin_queue_expiry_wait', () => barrier.waitPast(intent.receipt.queue_deadline));
        ctx.assert(Date.parse(elapsed.observed_at) > Date.parse(intent.receipt.queue_deadline),
          'admin_queue_expiry_not_elapsed');
      } else await f.revokeControl();
      await barrier.release();
      const result = await settle(ctx, f, 'creator', intent, 'rejected');
      ctx.assert(result.state.reason === mode, 'admin_sandbox_boundary_wrong_reason');
    }
  });
  await ctx.step('expected_result', async () => {
    const current = await sandboxRead(ctx, f, 'creator');
    ctx.assert(current.status === 'active', 'admin_rejected_command_changed_lifecycle');
    const effects = await ctx.observe('admin_sandbox_lifecycle_effects', () => f.readCommandEffects(ctx));
    ctx.assert(effects.pause === 1 && effects.revive === 1 && effects.retire === 0,
      'admin_sandbox_command_duplicate_effect');
  });
});

export const orgLifecycle = define(94, 'agent', 3, async ctx => {
  const f = await setup(ctx, 'organization_lifecycle');
  required(f.sandboxId && f.keys && typeof f.readProviderResources === 'function',
    'admin_org_lifecycle_fixture_missing');
  await ctx.step('step_1', async () => {
    capabilities(ctx, await access(ctx, f, 'org_admin', 'sandbox', f.sandboxId), ['manage_sandbox']);
    for (const [kind, status] of [['pause', 'paused'], ['revive', 'active']]) {
      const intent = await command(ctx, f, 'org_admin', kind, f.keys[kind]);
      await settle(ctx, f, 'org_admin', intent);
      const value = await poll(() => sandboxRead(ctx, f, 'org_admin'), s => s.status === status,
        'admin_org_lifecycle_timeout');
      ctx.assert(value.status === status, 'admin_org_lifecycle_not_observed');
    }
  });
  await ctx.step('step_2', async () => {
    capabilities(ctx, await access(ctx, f, 'org_admin', 'sandbox', f.sandboxId), ['manage_sharing']);
    const intent = await command(ctx, f, 'org_admin', 'retire', f.keys.retire);
    await settle(ctx, f, 'org_admin', intent);
    const value = await poll(() => sandboxRead(ctx, f, 'org_admin', other(ctx.am)), s => s.status === 'retired',
      'admin_org_retire_timeout');
    ctx.assert(value.id === f.sandboxId, 'admin_org_retire_wrong_identity');
    const cleanup = await ctx.observe('admin_org_provider_finality', () => f.readProviderResources(ctx));
    ctx.assert(cleanup.runtime_present === false && cleanup.test_owned_resources_remaining === 0,
      'admin_org_retirement_not_complete');
  });
  await ctx.step('step_3', async () => {
    // This is a separate live org_roles recipient Sandbox, so denial cannot pass merely because the target retired.
    const target = {...f, sandboxId: f.recipientSandboxId, agentId: f.recipientAgentId};
    capabilities(ctx, await access(ctx, target, 'org_member', 'sandbox', target.sandboxId),
      ['view_details'], ['manage_sharing']);
    const initial = await sandboxRead(ctx, target, 'creator');
    await roleRequest(ctx, target, 'org_member', 'POST', '/api/v1/sandboxes/' + id(target.sandboxId) + '/commands',
      {command: 'retire', expected_revision: initial.revision},
      {agentId: target.agentId, key: f.keys.denied, allowed: [403]});
    ctx.assert(equal(initial, await sandboxRead(ctx, target, 'creator', other(ctx.am))),
      'admin_org_recipient_retired_resource');
  });
  await ctx.step('expected_result', async () => {
    const effects = await ctx.observe('admin_org_lifecycle_effects', () => f.readCommandEffects(ctx));
    ctx.assert(effects.pause === 1 && effects.revive === 1 && effects.retire === 1, 'admin_org_duplicate_lifecycle');
  });
});
