import { randomUUID } from 'node:crypto';
import { Blocked } from '../../runner.mjs';
import { digest } from './helpers.mjs';
import { identity } from './setup-browser.mjs';

const enc = encodeURIComponent;
export const otherAM = (am) => (am === 'a' ? 'b' : 'a');

export function requireResourceInputs(primitives) {
  const config = primitives.userResources;
  if (
    config?.scope !== 'LOCAL_DISPOSABLE_TWO_AM' ||
    config.database_disposable !== true ||
    config.single_matrix_writer !== true ||
    typeof config.reserve !== 'function' ||
    typeof config.finality !== 'function'
  )
    throw new Blocked('user_resource_custody_required');
  if (
    !config.sandbox_profile?.id ||
    !Number.isSafeInteger(config.sandbox_profile.revision) ||
    config.sandbox_profile.revision < 1
  )
    throw new Blocked('user_resource_profile_required');
  if (
    config.timeout_ms !== undefined &&
    (!Number.isSafeInteger(config.timeout_ms) ||
      config.timeout_ms < 2000 ||
      config.timeout_ms > 300000)
  )
    throw new Blocked('user_resource_deadline_invalid');
  return config;
}

// All resources are created through public APIs. The parent lease records an
// intent BEFORE network I/O and retains custody if any result is uncertain.
export async function resources(ctx, primitives) {
  const config = requireResourceInputs(primitives);
  const lease = await config.reserve({
    purpose: 'user17-public-resources',
    caseId: ctx.row.case_id,
  });
  if (
    lease?.parent_cleanup_barrier !== true ||
    typeof lease.retainIntent !== 'function' ||
    typeof lease.retainResource !== 'function' ||
    typeof lease.release !== 'function'
  )
    throw new Blocked('user_resource_parent_barrier_required');
  const roots = [],
    uncertain = new Set();
  let cleanupPromise;
  const timeout = config.timeout_ms ?? 240000;
  async function read(path, { actor = 'user', agentId, am = otherAM(ctx.am) } = {}) {
    const response = await ctx.request({ actor, am, method: 'GET', path, agentId, allowed: [200] });
    ctx.assert(
      response.timing?.am === am && response.timing.request_id?.trim(),
      'setup_read_actual_am',
    );
    return response.body;
  }
  async function wait(readValue, predicate) {
    const end = Date.now() + timeout;
    do {
      const value = await readValue();
      if (predicate(value)) return value;
      if (Date.now() >= end) break;
      await new Promise((resolve) => setTimeout(resolve, 2000));
    } while (Date.now() < end);
    throw Error('user_setup_resource_deadline');
  }
  async function create(kind, path, body, { actor = 'user', parentId } = {}) {
    const key = randomUUID();
    const intent = { kind, actor, am: ctx.am, method: 'POST', path, key, body, parentId };
    await lease.retainIntent(intent);
    uncertain.add(key);
    let captured = false;
    const capture = async (response) => {
      if (
        captured ||
        ![201, 202].includes(response?.status) ||
        typeof response.body?.id !== 'string' ||
        !response.body.id.trim()
      )
        return;
      captured = true;
      const entry = { kind, id: response.body.id, actor, name: body.name, parentId, key };
      // Keep in-process cleanup custody even if the parent evidence write fails.
      if (kind === 'sandbox') roots.push(entry);
      await lease.retainResource(entry);
      uncertain.delete(key);
    };
    try {
      const response = await ctx.request({
        actor,
        am: ctx.am,
        method: 'POST',
        path,
        body,
        key,
        allowed: [kind === 'operation' ? 202 : 201],
      });
      await capture(response);
      ctx.assert(captured && !uncertain.has(key), 'user_setup_create_identity');
      ctx.assert(
        response.timing?.am === ctx.am && response.timing.request_id?.trim(),
        'setup_write_actual_am',
      );
      return response.body;
    } catch (error) {
      // The runner preserves an accepted response even if attribution rejects it.
      await capture(error.response);
      throw error;
    } finally {
      await ctx.observe('user_setup_create_custody', async () => ({
        kind,
        key_sha256: digest(key),
        captured,
        unresolved: uncertain.has(key),
      }));
    }
  }
  async function cleanupOnce() {
    const failures = [];
    for (const root of [...roots].reverse()) {
      try {
        const path = '/api/v1/sandboxes/' + enc(root.id);
        const options = { actor: root.actor };
        const current = await read(path, options);
        ctx.assert(
          current.id === root.id &&
            current.name === root.name &&
            current.owner === identity(primitives, root.actor).subject,
          'user_setup_cleanup_authority',
        );
        if (!['retiring', 'retired'].includes(current.status))
          await ctx.request({
            actor: root.actor,
            am: otherAM(ctx.am),
            method: 'DELETE',
            path,
            key: randomUUID(),
            allowed: [200, 202, 204],
          });
        await wait(
          () => read(path, options),
          (value) => value.id === root.id && value.status === 'retired',
        );
        const final = await config.finality({ sandboxId: root.id });
        ctx.assert(
          final?.sandbox_id === root.id &&
            final.children_remaining === 0 &&
            final.pending_operations === 0 &&
            final.cleanup_complete === true,
          'user_setup_cleanup_finality',
        );
      } catch (error) {
        failures.push(error);
      }
    }
    if (uncertain.size) failures.push(Error('user_setup_uncertain_creation_retained'));
    if (failures.length) throw new AggregateError(failures, 'user_setup_cleanup_unresolved');
    const released = await lease.release();
    ctx.assert(released?.released === true, 'user_setup_lease_release');
  }
  const cleanup = () => (cleanupPromise ??= cleanupOnce());
  ctx.defer('cleanup_partial_user_resources', cleanup);
  async function sandbox(actor = 'user') {
    identity(primitives, actor);
    const name = 'coverage17-' + randomUUID();
    const result = await create(
      'sandbox',
      '/api/v1/sandboxes',
      { name, sandbox_profile: config.sandbox_profile },
      { actor },
    );
    return wait(
      () => read('/api/v1/sandboxes/' + enc(result.id), { actor }),
      (current) => {
        ctx.assert(
          current.id === result.id &&
            current.name === name &&
            current.owner === identity(primitives, actor).subject,
          'user_setup_sandbox_identity',
        );
        ctx.assert(
          !['failed', 'retired', 'retiring'].includes(current.status),
          'user_setup_sandbox_failed',
        );
        return current.status === 'active';
      },
    );
  }
  async function hierarchy() {
    const parent = await sandbox();
    const created = await create(
      'workspace',
      `/api/v1/sandboxes/${enc(parent.id)}/workspaces/inline`,
      { name: 'coverage17-workspace-' + randomUUID(), source: { type: 'empty' } },
      { parentId: parent.id },
    );
    const workspace = await wait(
      () => read('/api/v1/workspaces/' + enc(created.id)),
      (value) => {
        ctx.assert(
          value.id === created.id && value.sandbox_id === parent.id,
          'user_setup_workspace_parent',
        );
        ctx.assert(!['failed', 'removed'].includes(value.status), 'user_setup_workspace_failed');
        return value.status === 'ready';
      },
    );
    const historyText = 'Local coverage history ' + randomUUID() + '. Reply only OK.';
    const name = 'coverage17-agent-' + randomUUID();
    const operation = await create(
      'operation',
      '/api/v1/agents',
      {
        name,
        workspace_id: workspace.id,
        machine_included: false,
        desktop: false,
        initial_message: historyText,
      },
      { parentId: parent.id },
    );
    const done = await wait(
      () => read('/api/v1/agent-lifecycle/' + enc(operation.id)),
      (value) => {
        ctx.assert(value.id === operation.id, 'user_setup_lifecycle_identity');
        ctx.assert(
          !['failed', 'waiting_for_connection'].includes(value.status),
          'user_setup_lifecycle_failed',
        );
        return value.status === 'succeeded';
      },
    );
    ctx.assert(
      typeof done.agent_id === 'string' && done.agent_id.trim(),
      'user_setup_agent_identity',
    );
    const agentId = done.agent_id;
    await lease.retainResource({
      kind: 'agent',
      id: agentId,
      actor: 'user',
      parentId: parent.id,
      operationId: operation.id,
      name,
    });
    const path = '/api/v1/agents/' + enc(agentId);
    const agent = await wait(
      () => read(path, { agentId }),
      (value) => {
        ctx.assert(
          value.id === agentId &&
            value.sandbox?.id === parent.id &&
            value.workspace?.id === workspace.id,
          'user_setup_agent_parent',
        );
        ctx.assert(
          !['failed', 'deleted', 'stopped'].includes(value.status),
          'user_setup_agent_failed',
        );
        return value.status === 'ready';
      },
    );
    await wait(
      () => read(path + '/transcript', { agentId }),
      (value) => {
        ctx.assert(
          value.agent_id === agentId && Array.isArray(value.turns) && value.next === undefined,
          'user_setup_transcript_identity',
        );
        const matches = value.turns.filter((turn) => turn.owner_message === historyText);
        ctx.assert(matches.length <= 1, 'user_setup_history_duplicate');
        const outcome = matches[0]?.outcome?.type;
        ctx.assert(
          !['failed', 'interrupted', 'cancelled'].includes(outcome),
          'user_setup_history_failed',
        );
        return outcome === 'completed';
      },
    );
    return { sandbox: parent, workspace, agent, agentId, agentName: name, historyText };
  }
  return { sandbox, hierarchy, read, cleanup };
}
