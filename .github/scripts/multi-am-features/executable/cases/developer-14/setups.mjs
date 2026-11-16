import {check, requireFixture, poll, unique, id, sha, read} from './common.mjs';

// The parent supplies only bounded infrastructure operations and exact runtime
// selection. Creation, readiness, public cleanup and capability reads live here.
// No callback may return a case verdict or fabricate an AM response.
export async function freshAgent(ctx, bindings, variant = 'supported') {
  requireFixture(bindings?.scope === 'LOCAL_DISPOSABLE_TWO_AM'
    && typeof bindings.acquireVariant === 'function'
    && typeof bindings.runtimeFor === 'function'
    && typeof bindings.registerWitness === 'function', 'configuration_runtime_binding_missing');
  const lease = await bindings.acquireVariant({variant, caseId: ctx.row.case_id});
  requireFixture(typeof lease?.release === 'function', 'configuration_variant_cleanup_missing');
  let leaseReleased = false;
  const releaseLease = async () => { if (!leaseReleased) { await lease.release(); leaseReleased = true; } };
  ctx.defer('configuration_runtime_lease_release', releaseLease);
  requireFixture(lease.scope === 'LOCAL_DISPOSABLE_TWO_AM' && lease.exclusive === true
    && /@sha256:[a-f0-9]{64}$/.test(lease.runtimeImage ?? '')
    && /^[a-f0-9]{40}$/.test(lease.runtimeSource ?? '') && typeof lease.owner === 'string',
  'configuration_exact_runtime_variant_required');
  requireFixture(lease.variant === variant, 'configuration_runtime_variant_mismatch');
  requireFixture(!lease.workspaceId, 'configuration_fresh_sandbox_required');
  const controls = bindings.provider;
  requireFixture(['prepareTurn', 'releaseTurn', 'oracle'].every(k => typeof controls?.[k] === 'function'),
    'configuration_provider_controls_missing');
  const record = {scope: lease.scope, createdByRun: true, custodyRegistered: true,
    name: unique('c275-config'), key: unique('create'), actor: 'user', controls};
  // Register custody before the creation attempt, including accepted requests
  // whose attribution later fails. Never retry or rekey the creation intent.
  let cleaned = false;
  record.cleanup = async () => { if (!cleaned) { await cleanup(ctx, record, lease.owner); cleaned = true; await releaseLease(); } };
  ctx.defer('configuration_public_agent_cleanup', record.cleanup);
  const body = {name: record.name, machine_included: false, desktop: false};
  const retainReceipt = response => { if (typeof response?.body?.id === 'string') record.operationId = response.body.id; };
  try {
    retainReceipt(await ctx.request({actor: 'user', am: ctx.am, method: 'POST', path: '/api/v1/agents',
      body, key: record.key, allowed: [202]}));
  } catch (error) {
    retainReceipt(error.response); record.unresolvedCreate = !record.operationId; throw error;
  }
  check(record.operationId, 'configuration_create_receipt_missing');
  await poll(async () => {
    const response = await ctx.request({actor: 'user', am: ctx.am, method: 'GET',
      path: '/api/v1/agent-lifecycle/' + id(record.operationId)});
    const value = response.body;
    check(value.id === record.operationId, 'configuration_lifecycle_identity');
    if (value.agent_id) record.agentId = value.agent_id;
    check(!['failed', 'waiting_for_connection'].includes(value.status), 'configuration_create_failed');
    return value;
  }, value => value.status === 'succeeded' && record.agentId, 'configuration_create_timeout', 420);
  const detail = await poll(() => read(ctx, record), value => {
    if (value.sandbox?.id) record.sandboxId = value.sandbox.id;
    check(value.id === record.agentId && !['failed', 'deleted'].includes(value.status), 'configuration_created_agent_invalid');
    return value.status === 'ready' && value.sandbox?.status === 'active'
      && (!value.first_message || value.first_message.state === 'completed');
  }, 'configuration_ready_timeout', 180);
  record.sandboxId = detail.sandbox.id;
  const runtime = await bindings.runtimeFor({agentId: record.agentId});
  check((runtime.agent_id === record.agentId || runtime.agent_sha256 === sha(record.agentId))
    && runtime.image === lease.runtimeImage && runtime.ready === true && runtime.pod_uid,
  'configuration_runtime_identity_mismatch');
  const preferences = await read(ctx, record, '/preferences');
  requireFixture(preferences.prompt_supported === (variant === 'supported')
    && preferences.prompt_pending === false, 'configuration_actual_runtime_capability_unavailable');
  await ctx.observe('configuration_runtime_binding', async () => ({agent_sha256: sha(record.agentId),
    image: runtime.image, source: lease.runtimeSource, pod_uid_sha256: sha(runtime.pod_uid),
    prompt_supported: preferences.prompt_supported, preferences_revision: preferences.revision}));
  await bindings.registerWitness({agentId: record.agentId});
  return record;
}

export async function cleanup(ctx, record, owner) {
  check(!record.unresolvedCreate, 'configuration_unknown_create_outcome_requires_parent_cleanup');
  if (!record.operationId) return;
  const op = (await ctx.request({actor: 'user', am: ctx.am, method: 'GET',
    path: '/api/v1/agent-lifecycle/' + id(record.operationId)})).body;
  check(op.id === record.operationId, 'configuration_cleanup_operation_mismatch');
  record.agentId ??= op.agent_id;
  check(record.agentId, 'configuration_cleanup_agent_not_resolved');
  const detail = await read(ctx, record);
  check(detail.id === record.agentId, 'configuration_cleanup_agent_identity');
  record.sandboxId ??= detail.sandbox?.id;
  check(record.sandboxId, 'configuration_cleanup_sandbox_unresolved');
  const path = '/api/v1/sandboxes/' + id(record.sandboxId);
  const sandbox = (await ctx.request({actor: 'user', method: 'GET', path, agentId: record.agentId})).body;
  check(sandbox.id === record.sandboxId && sandbox.owner === owner, 'configuration_cleanup_owner_mismatch');
  const suffix = ' sandbox ' + record.operationId;
  check(sandbox.name === record.name.slice(0, Math.max(0, 64 - suffix.length)) + suffix,
    'configuration_cleanup_custody_mismatch');
  // Retiring this freshly owned Sandbox cascades through its owned Agent and
  // workspace. The parent observes final retirement after ownership is gone.
  await ctx.request({actor: 'user', method: 'DELETE', path, agentId: record.agentId,
    key: record.key + '-retire', allowed: [200, 202, 204]});
  await poll(async () => ctx.request({actor: 'user', am: ctx.am, method: 'GET', path, allowed: [200, 404]}),
    r => r.status === 404 || r.body?.status === 'retired', 'configuration_public_retire_timeout', 300);
}

const create = (ctx, primitives, variant) => freshAgent(ctx,
  {...primitives['configuration-runtime'], provider: primitives['provider-controls']}, variant);
export const setups = {
  'agent-rename': (ctx, primitives) => create(ctx, primitives, 'supported'),
  'agent-system-prompt': (ctx, primitives) => create(ctx, primitives, 'supported'),
  'agent-system-prompt-unsupported': (ctx, primitives) => create(ctx, primitives, 'unsupported'),
  'agent-session-settings': (ctx, primitives) => create(ctx, primitives, 'supported'),
};
