import {isDeepStrictEqual as equal} from 'node:util';
import {assertOwner, check, hash, id, other, poll} from '../cma/core.mjs';

export async function runtimeLoss({adapter, api, record, openBoth, complete, waitFor = poll}) {
  const {agentId, sandboxId, operationId} = record;
  check([agentId, sandboxId, operationId].every(id), 'runtime_loss_record_invalid');
  const agentPath = '/api/v1/agents/' + agentId;
  const read = async (am, path) => (await api.rawRequest(am, 'GET', path)).body;
  const snapshot = () => adapter.lifecycleSnapshot({sandboxId, agentIds: [agentId]});
  const priorOwner = assertOwner(await adapter.ownerOf({agentId}), agentId);
  const reader = other(priorOwner.am);
  const detail = await read(reader, agentPath);
  const history = await read(reader, agentPath + '/transcript');
  const before = await snapshot();
  const agent = before?.children?.[0];
  const runtime = await adapter.runtimeFor({agentId});
  check(detail?.id === agentId && detail?.sandbox?.id === sandboxId && id(detail.workspace?.id)
    && id(detail.worktree?.id) && detail.status === 'ready' && history?.agent_id === agentId
    && history.turns?.some(turn => turn.outcome?.type === 'completed' && turn.messages?.length > 0)
    && before?.status === 'active' && agent?.status === 'ready' && id(before.boot_id)
    && id(agent.conversation_id) && Number.isSafeInteger(agent.runtime_generation) && agent.runtime_generation > 0
    && Number.isSafeInteger(agent.last_seq) && runtime.ready === true && runtime.sandbox_id === sandboxId
    && id(runtime.pod_uid) && runtime.image === record.runtime.image && id(runtime.home_identity)
    && Array.isArray(runtime.pvc_names) && runtime.pvc_names.length > 0, 'runtime_loss_baseline_unbound');

  const removed = await adapter.removeRuntimePod({agentId, sandboxId, operationId, podUid: runtime.pod_uid});
  check(removed?.accepted === true && removed.pod_uid === runtime.pod_uid, 'runtime_loss_removal_unbound');
  const recovered = await waitFor(snapshot, value => {
    const candidate = value?.children?.[0];
    check(candidate && value.status !== 'failed' && !['failed', 'deleted', 'stopped'].includes(candidate.status),
      'runtime_loss_recovery_terminal');
    return value.status === 'active' && candidate.status === 'ready' && id(value.boot_id)
      && value.boot_id !== before.boot_id && candidate.runtime_generation > agent.runtime_generation;
  }, {timeout: 240000, code: 'runtime_loss_recovery_timeout'});
  check(recovered.children[0].conversation_id === agent.conversation_id, 'runtime_loss_conversation_replaced');
  const replacement = await adapter.runtimeFor({agentId});
  check(replacement.ready === true && id(replacement.pod_uid) && replacement.pod_uid !== runtime.pod_uid
    && replacement.image === runtime.image && replacement.sandbox_id === sandboxId
    && replacement.home_identity === runtime.home_identity && Array.isArray(replacement.pvc_names)
    && equal([...replacement.pvc_names].sort(), [...runtime.pvc_names].sort()), 'runtime_loss_home_replaced');
  check((await openBoth(record)).both_am_details_and_transcripts === true, 'runtime_loss_open_unconfirmed');
  const settledOwner = assertOwner(await adapter.ownerOf({agentId}), agentId);
  await complete(record, other(settledOwner.am));
  for (const am of ['a', 'b']) {
    const current = await read(am, agentPath);
    const transcript = await read(am, agentPath + '/transcript');
    const {items, next} = await read(am, agentPath + '/events?after=' + agent.last_seq);
    check(current.id === agentId && current.sandbox.id === sandboxId && current.status === 'ready'
      && current.workspace.id === detail.workspace.id && current.worktree.id === detail.worktree.id,
    'runtime_loss_hierarchy_changed');
    check(transcript?.agent_id === agentId && history.turns.every(turn =>
      transcript.turns.some(currentTurn => equal(currentTurn, turn))), 'runtime_loss_history_changed');
    const replacements = items?.filter(entry => entry.kind?.type === 'runtime_generation_retired');
    check(next === undefined && replacements?.length === 1
      && replacements[0].kind.previous_generation === agent.runtime_generation
      && replacements[0].kind.runtime_generation === recovered.children[0].runtime_generation
      && recovered.children[0].runtime_generation === agent.runtime_generation + 1,
    'runtime_loss_generation_retirement_unbound');
  }
  return {previous_pod_sha256: hash(runtime.pod_uid), replacement_pod_sha256: hash(replacement.pod_uid),
    agent_sha256: hash(agentId), retained_runtime_image: true, retained_home: true,
    retained_conversation: true, generation_retirements: 1, post_loss_reply: true};
}
