import {isDeepStrictEqual} from 'node:util';
import {check, hash, id, other, poll} from '../cma/core.mjs';
import {ownerlessRevival} from './ownerless_revival.mjs';

export async function retainedLifecycle({api, adapter, record, create, cleanup, step, beforePause, ...journey}) {
  check(id(record.workspaceId) && id(record.sandboxId), 'pause_parent_lineage_required');
  const sibling = await create(other(record.am), false, {workspace_id: record.workspaceId});
  check(sibling.sandboxId === record.sandboxId && sibling.workspaceId === record.workspaceId &&
    sibling.agentId !== record.agentId, 'pause_sibling_lineage_changed');
  await beforePause?.(sibling);
  const read = () => adapter.lifecycleSnapshot({sandboxId: record.sandboxId,
    agentIds: [record.agentId, sibling.agentId]});
  const before = await read();
  const priorRuntime = await adapter.runtimeFor({agentId: record.agentId});
  await step('ownerless_pause_revive_open', () => ownerlessRevival({api, adapter, record, ...journey,
    pauseObservation: async () => {
      await step('two_agent_pause_all_or_none', () => observeAtomicPause({read, before}));
      await step('pause_preserves_retained_home_authority', async () => assertRetainedHome(before, await read(), priorRuntime));
    }}));
  await step('revival_reattaches_retained_home', async () => {
    const runtime = await adapter.runtimeFor({agentId: record.agentId});
    check(runtime.ready === true && runtime.home_identity === priorRuntime.home_identity
      && runtime.pod_uid !== priorRuntime.pod_uid && id(runtime.pod_uid)
      && Array.isArray(runtime.pvc_names) && isDeepStrictEqual([...runtime.pvc_names].sort(), [...priorRuntime.pvc_names].sort()),
    'revival_retained_home_changed');
    return {retained_home_sha256: hash(runtime.home_identity), volume_claims: runtime.pvc_names.length};
  });
  await step('two_agent_revival_restores_both', () => verifySiblingRecovery({api, adapter, record,
    sibling, openBoth: journey.openBoth}));
  await cleanup(sibling);
}

export function assertRetainedHome(before, paused, priorRuntime) {
  check(paused?.status === 'paused' && paused.runtime_detached === true && priorRuntime.ready === true
    && id(priorRuntime.home_identity) && id(priorRuntime.pod_uid)
    && Array.isArray(priorRuntime.pvc_names) && priorRuntime.pvc_names.length > 0
    && Number.isSafeInteger(before.provision_generation) && before.provision_generation > 0
    && paused.retained_home?.identity === priorRuntime.home_identity
    && paused.retained_home.source_generation === before.provision_generation
    && paused.checkpoint_epoch === before.checkpoint_epoch, 'pause_retained_home_unbound');
  const prior = before.children;
  const children = paused.children;
  check(Array.isArray(prior) && prior.length === 2 && Array.isArray(children) && children.length === 2
    && new Set(prior.map(child => child?.agent_id)).size === prior.length
    && prior.every((child, index) => id(child?.agent_id) && id(child.conversation_id)
      && children[index]?.agent_id === child.agent_id && children[index].status === 'paused'
      && children[index].conversation_id === child.conversation_id), 'pause_retained_home_child_lineage_changed');
  return {retained_home_sha256: hash(paused.retained_home.identity),
    source_generation: paused.retained_home.source_generation, retained_conversations: children.length};
}

export async function observeAtomicPause({read, before, waitFor = poll}) {
  check(before?.status === 'active' && Array.isArray(before.children) && before.children.length === 2 &&
    before.children.every(row => row?.status === 'ready' && Number.isSafeInteger(row.last_seq) &&
      Number.isSafeInteger(row.revision)) && Number.isSafeInteger(before.revision), 'pause_baseline_unbound');
  let samples = 0;
  await waitFor(async () => {
    const sample = await read();
    check(sample && ['active', 'pausing', 'paused'].includes(sample.status) &&
      Number.isSafeInteger(sample.revision) && sample.revision >= before.revision &&
      Array.isArray(sample.children) && sample.children.length === before.children.length &&
      sample.children.every((row, index) => row && Number.isSafeInteger(row.revision) &&
        Number.isSafeInteger(row.last_seq) && row.revision >= before.children[index].revision &&
        row.last_seq >= before.children[index].last_seq), 'pause_snapshot_lineage_changed');
    const statuses = sample.children.map(row => row.status);
    check(statuses.every(status => status === statuses[0]) && ['ready', 'paused'].includes(statuses[0]),
      'pause_published_partial_hierarchy');
    check(sample.status !== 'paused' || statuses[0] === 'paused', 'pause_published_parent_before_children');
    samples += 1;
    return sample;
  }, sample => sample.status === 'paused' && sample.revision > before.revision &&
    sample.children.every((row, index) => row.revision > before.children[index].revision &&
      row.last_seq > before.children[index].last_seq), {timeout: 240000, code: 'pause_atomic_publication_missing'});
  return {snapshot_samples: samples, paused_agents: before.children.length};
}

export async function verifySiblingRecovery({api, adapter, record, sibling, openBoth}) {
  const read = async () => adapter.lifecycleSnapshot({sandboxId: record.sandboxId,
    agentIds: [record.agentId, sibling.agentId]});
  const snapshot = await read();
  check(snapshot?.status === 'active' && snapshot.children?.length === 2 &&
    snapshot.children.every(child => child?.status === 'ready'), 'revival_sibling_not_restored');
  const opened = await openBoth(sibling);
  check(opened?.both_am_details_and_transcripts === true, 'revival_sibling_open_missing');
  const operation = `/api/v1/agent-lifecycle/${sibling.operationId}`;
  for (const am of ['a', 'b']) {
    const {body} = await api.rawRequest(am, 'GET', operation);
    check(body?.id === sibling.operationId && body.agent_id === sibling.agentId && body.status === 'succeeded',
      'revival_sibling_operation_changed');
  }
  return {sibling_sha256: hash(sibling.agentId), restored_agent_count: snapshot.children.length};
}
