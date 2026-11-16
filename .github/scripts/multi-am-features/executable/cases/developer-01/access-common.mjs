import {createHash} from 'node:crypto';
import {roleRequest, id, required, check, equal} from './common.mjs';
import {poll} from './enforcement.mjs';

export const digest = value => createHash('sha256').update(value).digest('hex');
export const agentPath = f => '/api/v1/agents/' + id(f.agentId);
export async function agentRequest(ctx, f, role, method, suffix = '', body, options = {}) {
  return roleRequest(ctx, f, role, method, agentPath(f) + suffix, body, {agentId: f.agentId, ...options});
}
export async function access(ctx, f, role, kind = 'agent', target = f.agentId) {
  return (await roleRequest(ctx, f, role, 'GET', '/api/v1/resource-access/' + kind + '/' + id(target),
    undefined, {agentId: f.agentId})).body;
}
export function capabilities(ctx, value, allowed, denied = []) {
  ctx.assert(Array.isArray(value.capabilities) && allowed.every(c => value.capabilities.includes(c))
    && denied.every(c => !value.capabilities.includes(c)), 'admin_effective_capability_mismatch');
}
export async function transcript(ctx, f, role) {
  return (await agentRequest(ctx, f, role, 'GET', '/transcript')).body;
}
export function markerCount(value, marker) {
  check(typeof marker === 'string' && marker.length >= 8, 'admin_unique_marker_missing');
  return JSON.stringify(value).split(marker).length - 1;
}
export async function interaction(ctx, f, role) {
  required(f.inputFirst && f.inputSecond && f.markers?.first && f.markers?.second
    && typeof f.waitTurnStarted === 'function' && typeof f.waitApproval === 'function',
  'admin_interaction_fixture_missing');
  capabilities(ctx, await access(ctx, f, role), ['interact', 'resolve_approval']);
  const first = await agentRequest(ctx, f, role, 'POST', '/input', f.inputFirst,
    {key: f.keys.first, allowed: [200, 202]});
  ctx.assert(first.status < 300, 'admin_first_send_admission');
  const running = await ctx.observe('admin_first_turn_started', () => f.waitTurnStarted(ctx));
  ctx.assert(running.agent_id === f.agentId && running.active === true, 'admin_first_turn_not_started');
  await agentRequest(ctx, f, role, 'POST', '/interrupt', f.interruptBody, {key: f.keys.interrupt, allowed: [200, 202]});
  const idle = await poll(() => f.readTurnState(ctx), v => v.active === false, 'admin_interrupt_timeout');
  ctx.assert(idle.turn_id === running.turn_id && idle.interrupted === true, 'admin_interrupt_wrong_turn');
  await agentRequest(ctx, f, role, 'POST', '/input', f.inputSecond, {key: f.keys.second, allowed: [200, 202]});
  const approval = await ctx.observe('admin_real_approval', () => f.waitApproval(ctx));
  required(approval.request_id && approval.agent_id === f.agentId, 'admin_approval_identity_missing');
  await agentRequest(ctx, f, role, 'POST', '/approvals/' + id(approval.request_id), f.approvalBody,
    {key: f.keys.approval, allowed: [200, 202]});
  const history = await poll(() => transcript(ctx, f, role), value => markerCount(value, f.markers.second) > 0,
    'admin_second_send_timeout');
  ctx.assert(markerCount(history, f.markers.first) === 1 && markerCount(history, f.markers.second) === 1,
    'admin_interaction_lost_or_duplicate_output');
  const effects = await ctx.observe('admin_interaction_effects', () => f.readProviderEffects(ctx));
  ctx.assert(effects.approval_ids.filter(v => v === approval.request_id).length === 1,
    'admin_approval_duplicate_effect');
}

export async function revokedSend(ctx, f, role, revoke) {
  required(typeof f.holdBeforeEffect === 'function' && typeof revoke === 'function',
    'admin_authority_revocation_barrier_missing');
  const barrier = await f.holdBeforeEffect();
  ctx.defer('admin_effect_barrier_cleanup', () => barrier.cleanup());
  const before = await f.readProviderEffects(ctx);
  await agentRequest(ctx, f, role, 'POST', '/input', f.inputRevoked,
    {key: f.keys.revoked, allowed: [200, 202]});
  await barrier.waitForAdmission(); await revoke(); await barrier.release();
  const result = await ctx.observe('admin_revoked_command_receipt', () => f.waitRevokedReceipt(ctx));
  ctx.assert(result.status === 'rejected' && ['authority_revoked', 'access_revoked'].includes(result.reason),
    'admin_revoked_command_not_rejected');
  const after = await f.readProviderEffects(ctx);
  ctx.assert(equal(before, after), 'admin_revoked_command_reached_provider');
}
