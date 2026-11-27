import {isDeepStrictEqual as equal} from 'node:util';
import {assertOwner, check, hash, id, other, poll} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

export async function runtimeLoss({adapter, api, record, sibling, openBoth, complete, waitFor = poll}) {
  const {agentId, sandboxId, operationId} = record;
  check([agentId, sandboxId, operationId, sibling?.agentId].every(id) && sibling.sandboxId === sandboxId
    && sibling.agentId !== agentId, 'runtime_loss_record_invalid');
  const agents = [record, sibling];
  const read = async (am, path) => (await api.rawRequest(am, 'GET', path)).body;
  const snapshot = () => adapter.lifecycleSnapshot({sandboxId, agentIds: agents.map(agent => agent.agentId)});
  const siblingOwner = assertOwner(await adapter.ownerOf({agentId: sibling.agentId}), sibling.agentId);
  await complete(sibling, other(siblingOwner.am));
  const baselines = [];
  for (const agent of agents) {
    const owner = assertOwner(await adapter.ownerOf({agentId: agent.agentId}), agent.agentId);
    const path = '/api/v1/agents/' + agent.agentId;
    const detail = await read(other(owner.am), path);
    const history = await read(other(owner.am), path + '/transcript');
    check(detail?.id === agent.agentId && detail.sandbox?.id === sandboxId && id(detail.workspace?.id)
      && id(detail.worktree?.id) && detail.status === 'ready' && history?.agent_id === agent.agentId
      && history.turns?.some(turn => turn.outcome?.type === 'completed' && turn.messages?.length > 0),
    'runtime_loss_history_baseline_unbound');
    baselines.push({path, detail, history});
  }
  const before = await snapshot();
  const runtime = await adapter.runtimeFor({agentId});
  check(before?.status === 'active' && id(before.boot_id) && before.children?.length === agents.length
    && before.children.every(agent => agent?.status === 'ready' && id(agent.conversation_id)
      && Number.isSafeInteger(agent.runtime_generation) && agent.runtime_generation > 0
      && Number.isSafeInteger(agent.last_seq)) && runtime.ready === true && runtime.sandbox_id === sandboxId
    && id(runtime.pod_uid) && runtime.image === record.runtime.image && id(runtime.home_identity)
    && Array.isArray(runtime.pvc_names) && runtime.pvc_names.length > 0, 'runtime_loss_baseline_unbound');

  const recoveredSnapshot = value => {
    check(value?.status !== 'failed' && value?.children?.length === agents.length
      && value.children.every(candidate => candidate && !['failed', 'deleted', 'stopped'].includes(candidate.status)),
    'runtime_loss_recovery_terminal');
    let advanced = 0;
    for (const [index, candidate] of value.children.entries()) {
      const baseline = before.children[index];
      check(candidate.conversation_id === baseline.conversation_id, 'runtime_loss_conversation_replaced');
      const generations = candidate.runtime_generation - baseline.runtime_generation;
      check((generations === 0 || generations === 1) && candidate.last_seq >= baseline.last_seq,
        'runtime_loss_generation_or_sequence_unbound');
      advanced += generations;
    }
    check(advanced === 0 || advanced === agents.length, 'runtime_loss_partial_generation_retirement');
    return value.status === 'active' && id(value.boot_id) && value.boot_id !== before.boot_id
      && advanced === agents.length && value.children.every(candidate => candidate.status === 'ready');
  };
  const observeAbort = () => adapter.runtimeLossEvidence({sandboxId});
  const priorAborts = await observeAbort();
  let armed, disarmed, observedAborts;
  await withRestoration(async () => {
    armed = await adapter.writerPublicationAbort({arm: true});
    const removed = await adapter.removeRuntimePod({agentId, sandboxId, operationId, podUid: runtime.pod_uid});
    check(removed?.accepted === true && removed.pod_uid === runtime.pod_uid, 'runtime_loss_removal_unbound');
    observedAborts = await waitFor(observeAbort, value => ['a', 'b'].some(am => value[am] > priorAborts[am]),
      {timeout: 90000, code: 'runtime_loss_abort_not_observed'});
  }, async () => {
    disarmed = await adapter.writerPublicationAbort({arm: false});
  });
  check(Number.isSafeInteger(armed?.count) && disarmed?.count === armed.count + 1,
    'runtime_loss_abort_not_consumed_once');
  const aborts = ['a', 'b'].map(am => observedAborts[am] - priorAborts[am]);
  check(aborts.every(count => count === 0 || count === 1) && aborts[0] + aborts[1] === 1,
    'runtime_loss_abort_count_unbound');
  const recovered = await waitFor(snapshot, recoveredSnapshot, {timeout: 240000, code: 'runtime_loss_recovery_timeout'});
  const replacement = await adapter.runtimeFor({agentId});
  check(replacement.ready === true && id(replacement.pod_uid) && replacement.pod_uid !== runtime.pod_uid
    && replacement.image === runtime.image && replacement.sandbox_id === sandboxId
    && replacement.home_identity === runtime.home_identity && Array.isArray(replacement.pvc_names)
    && equal([...replacement.pvc_names].sort(), [...runtime.pvc_names].sort()), 'runtime_loss_home_replaced');
  for (const [index, agent] of agents.entries()) {
    check((await openBoth(agent)).both_am_details_and_transcripts === true, 'runtime_loss_open_unconfirmed');
    const owner = assertOwner(await adapter.ownerOf({agentId: agent.agentId}), agent.agentId);
    await complete(agent, other(owner.am));
    const {path, detail, history} = baselines[index];
    const baseline = before.children[index];
    for (const am of ['a', 'b']) {
      const current = await read(am, path);
      const transcript = await read(am, path + '/transcript');
      const {items, next} = await read(am, path + '/events?after=' + baseline.last_seq);
      check(current.id === agent.agentId && current.sandbox.id === sandboxId && current.status === 'ready'
        && current.workspace.id === detail.workspace.id && current.worktree.id === detail.worktree.id,
      'runtime_loss_hierarchy_changed');
      check(transcript?.agent_id === agent.agentId && history.turns.every(turn =>
        transcript.turns.some(currentTurn => equal(currentTurn, turn))), 'runtime_loss_history_changed');
      const retirements = items?.filter(entry => entry.kind?.type === 'runtime_generation_retired');
      check(next === undefined && retirements?.length === 1
        && retirements[0].kind.previous_generation === baseline.runtime_generation
        && retirements[0].kind.runtime_generation === recovered.children[index].runtime_generation,
      'runtime_loss_generation_retirement_unbound');
    }
  }
  return {previous_pod_sha256: hash(runtime.pod_uid), replacement_pod_sha256: hash(replacement.pod_uid),
    recovered_agents: agents.length, retained_runtime_image: true, retained_home: true,
    retained_conversations: true, known_recovery_aborts: 1,
    generation_retirements: agents.length, post_loss_replies: agents.length};
}
