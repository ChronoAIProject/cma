import {check, hash, opposite, request, requireOwnerProof, unique, waitFor} from './helpers.mjs';
import {contextFor} from './context.mjs';

export const deletedAgent = response => [404, 410].includes(response.status)
  || (response.body?.status === 'deleted' && response.body.worktree?.status === 'removed');

async function terminalRead(ctx, state, mutation, path, accept, label) {
  const am = opposite(mutation.timing.am);
  const result = await waitFor(ctx, () => request(ctx, state.identity, {am, method: 'GET', path,
    allowed: [200, 404, 410]}), accept, label);
  check(result.timing.am === am && result.timing.request_id, label + '_peer_receipt');
  await ctx.observe(label, async () => ({mutation_request_id: mutation.timing.request_id,
    mutation_am: mutation.timing.am, terminal_request_id: result.timing.request_id,
    terminal_am: result.timing.am, terminal_status: result.body?.status ?? result.status}));
  return result;
}

export async function j5(ctx, state) {
  state.requestIds = [];
  // Capture terminal mutations via the parent's lifecycle worker witness: the
  // generic live-lease router cannot attribute an operation after lease release.
  const owner = await contextFor(ctx).adapter.ownerOf({agentId: state.agentId});
  check(['a', 'b'].includes(owner?.am), 'stop_live_owner_missing');
  const stop = await request(ctx, state.identity, {am: opposite(owner.am), method: 'POST',
    path: `/api/v1/agents/${state.agentId}/stop`, body: {reason: 'owner'},
    key: unique('stop'), allowed: [202], proofState: state.requestIds, lifecycle: true});
  await terminalRead(ctx, state, stop, `/api/v1/agents/${state.agentId}`,
    value => value.status === 200 && value.body?.status === 'stopped', 'stop_terminal_peer_read');
  ctx.assert(true, 'agent_exact_stopped');
  await requireOwnerProof(ctx, state, 'j5_stop_owner');
  const pause = await request(ctx, state.identity, {am: stop.timing.am, method: 'POST',
    path: `/api/v1/sandboxes/${state.sandboxId}/pause`, body: {}, key: unique('pause'),
    allowed: [202], proofState: state.requestIds, lifecycle: true});
  const paused = await terminalRead(ctx, state, pause, `/api/v1/sandboxes/${state.sandboxId}`,
    value => value.status === 200 && value.body?.status === 'paused', 'pause_terminal_peer_read');
  const revive = await request(ctx, state.identity, {am: opposite(pause.timing.am), method: 'POST',
    path: `/api/v1/sandboxes/${state.sandboxId}/revive`, body: {}, key: unique('revive'),
    allowed: [202], proofState: state.requestIds, lifecycle: true});
  const active = await terminalRead(ctx, state, revive, `/api/v1/sandboxes/${state.sandboxId}`,
    value => value.status === 200 && value.body?.status === 'active', 'resume_terminal_peer_read');
  ctx.assert(paused.body.id === state.sandboxId && active.body.id === state.sandboxId, 'pause_resume_lineage');
  await requireOwnerProof(ctx, state, 'j5_pause_resume_owner');
}

export async function j6(ctx, state) {
  state.requestIds = [];
  const current = await request(ctx, state.identity, {am: state.am, method: 'GET',
    path: `/api/v1/agents/${state.agentId}`, allowed: [200]});
  check(current.body?.status === 'stopped', 'delete_requires_exact_stopped');
  state.deleteAttempted = true;
  const deleted = await request(ctx, state.identity, {am: opposite(state.am), method: 'DELETE',
    path: `/api/v1/agents/${state.agentId}`, key: state.key + '-delete',
    allowed: [202], proofState: state.requestIds, lifecycle: true});
  await terminalRead(ctx, state, deleted, `/api/v1/agents/${state.agentId}`, deletedAgent, 'delete_terminal_peer_read');
  state.deleted = true;
  ctx.assert(state.deleted, 'delete_confirmed');
  await requireOwnerProof(ctx, state, 'j6_delete_owner');
  await ctx.observe('delete_result', async () => ({agent_sha256: hash(state.agentId), deleted: true}));
}
