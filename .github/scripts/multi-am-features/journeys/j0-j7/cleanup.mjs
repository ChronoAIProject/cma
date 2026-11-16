import {contextFor} from './context.mjs';
import {opposite, request, waitFor} from './helpers.mjs';
import {deletedAgent} from './transitions.mjs';

export async function cleanupAgent(ctx, state) {
  if (state.unresolvedCreate) throw Error('unknown_create_outcome_requires_parent_reconciliation');
  if (!state.agentId && state.operationId) {
    const lifecycle = await waitFor(ctx, () => request(ctx, state.identity, {am: opposite(state.am), method: 'GET',
      path: `/api/v1/agent-lifecycle/${state.operationId}`, allowed: [200]}), value => {
      state.agentId ??= value.body?.agent_id;
      return ['succeeded', 'failed', 'cancelled'].includes(value.body?.status);
    }, 'cleanup_lifecycle_reconciliation_timeout', {attempts: 30});
    if (!state.agentId && lifecycle.body?.status !== 'cancelled')
      throw Error('creation_lineage_requires_parent_reconciliation');
  }
  if (state.agentId) {
    const read = () => request(ctx, state.identity, {am: state.am, method: 'GET',
      path: `/api/v1/agents/${state.agentId}`, allowed: [200, 404, 410]});
    const current = await read();
    state.sandboxId ??= current.body?.sandbox?.id;
    if (!deletedAgent(current)) {
      if (state.deleteAttempted) {
        await waitFor(ctx, read, deletedAgent, 'unknown_delete_outcome_requires_parent_reconciliation', {attempts: 30});
      } else {
        if (!['stopped', 'stopping'].includes(current.body?.status)) {
          await request(ctx, state.identity, {am: state.am, method: 'POST',
            path: `/api/v1/agents/${state.agentId}/stop`, body: {reason: 'owner'},
            key: state.key + '-cleanup-stop', allowed: [202]});
        }
        await waitFor(ctx, read, value => value.body?.status === 'stopped', 'cleanup_stop_timeout', {attempts: 30});
        state.deleteAttempted = true;
        await request(ctx, state.identity, {am: state.am, method: 'DELETE',
          path: `/api/v1/agents/${state.agentId}`, key: state.key + '-delete', allowed: [202]});
        await waitFor(ctx, read, deletedAgent, 'cleanup_delete_timeout', {attempts: 30});
      }
    }
    state.deleted = true;
  }
  if (state.sandboxId) {
    const path = `/api/v1/sandboxes/${state.sandboxId}`;
    const read = () => request(ctx, state.identity, {am: opposite(state.am), method: 'GET', path, allowed: [200, 404, 410]});
    const terminal = value => [404, 410].includes(value.status) || value.body?.status === 'retired';
    if (!terminal(await read())) {
      await request(ctx, state.identity, {am: state.am, method: 'DELETE', path,
        key: state.key + '-cleanup-sandbox', allowed: [202]});
      await waitFor(ctx, read, terminal, 'cleanup_sandbox_timeout', {attempts: 30});
    }
  }
  contextFor(ctx).resourcesReleased = true;
}
