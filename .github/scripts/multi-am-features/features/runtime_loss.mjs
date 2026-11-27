import {isDeepStrictEqual as equal} from 'node:util';
import {randomUUID} from 'node:crypto';
import {assertOwner, check, hash, id, other, poll} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';
import {NonOwnerRouter} from '../routing.mjs';

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
  const observeRecovery = () => adapter.runtimeLossEvidence({sandboxId});
  const initialTopology = await adapter.topology();
  const managers = ['a', 'b'];
  const priorRecovery = await observeRecovery();
  check(managers.every(am => priorRecovery[am]?.known_aborts === 0
    && priorRecovery[am]?.recovery_scheduled === 0), 'runtime_loss_prior_recovery');
  const preferencePath = `/api/v1/agents/${sibling.agentId}/preferences`;
  const preferences = await read(siblingOwner.am, preferencePath);
  check(Number.isSafeInteger(preferences?.revision), 'runtime_loss_writer_baseline_unbound');
  const title = 'Runtime recovery fixture ' + randomUUID();
  let writer;
  let writerProof;
  const router = new NonOwnerRouter({adapter, record: proof => { writerProof = proof; }});
  const recoveryManager = other(siblingOwner.am);
  const inventoryAuthority = async allow => {
    const authority = await adapter.runtimeInventoryAuthority({manager: siblingOwner.am, allow});
    check(authority?.manager === siblingOwner.am && authority.allowed === allow && authority.peer_allowed === true,
      'runtime_loss_inventory_authority_unbound');
  };
  let armed, disarmed, observedRecovery, publication;
  const removeAndWrite = async () => {
    armed = await adapter.runtimeLossPublicationAbort({action: 'arm'});
    check(Number.isSafeInteger(armed?.count), 'runtime_loss_abort_count_unbound');
    const removed = await adapter.removeRuntimePod({agentId, sandboxId, operationId, podUid: runtime.pod_uid});
    check(removed?.accepted === true && removed.pod_uid === runtime.pod_uid, 'runtime_loss_removal_unbound');
    await waitFor(() => adapter.runtimeLossPublicationAbort({action: 'wait', count: armed.count}),
      value => value?.entered === true, {timeout: 90000, interval: 100, code: 'runtime_loss_transaction_not_observed'});
    writer = await router.dispatch(sibling.agentId, am => api.rawRequest(am, 'PATCH', preferencePath, {
      body: {title, expected_revision: preferences.revision}, key: randomUUID(), allowed: [200], timeout: 8000,
    }));
    check(writer.body?.title === title && writer.body?.revision === preferences.revision + 1,
      'runtime_loss_competing_publication_unbound');
    check(writerProof?.verified_owner_write === true && writer.timing.am === writerProof.ingress
      && writerProof.owner === siblingOwner.am && writerProof.owner_epoch === siblingOwner.epoch,
      'runtime_loss_writer_unattributed');
    const ingressEvidence = await adapter.peerWriteEvidence({am: writerProof.ingress,
      requestHash: hash(writerProof.request_id), epoch: writerProof.owner_epoch, operation: 'preferences'});
    check(ingressEvidence?.matches === 0, 'runtime_loss_writer_executed_on_ingress');
    check(equal(await observeRecovery(), priorRecovery), 'runtime_loss_writer_missed_transaction_window');
    observedRecovery = await waitFor(observeRecovery, value => managers.some(am => value[am]?.known_aborts > 0),
      {timeout: 90000, code: 'runtime_loss_abort_not_observed'});
  };
  await withRestoration(async () => {
    await inventoryAuthority(false);
    await withRestoration(removeAndWrite, async () => {
      disarmed = await adapter.runtimeLossPublicationAbort({action: 'disarm'});
    });
    publication = await waitFor(observeRecovery, value => managers.some(am => value[am]?.recovery_scheduled > 0),
      {timeout: 90000, code: 'runtime_loss_successful_publication_unattributed'});
  }, () => inventoryAuthority(true));
  check(Number.isSafeInteger(armed?.count) && disarmed?.count === armed.count + 1,
    'runtime_loss_abort_not_consumed_once');
  const aborts = managers.map(am => observedRecovery[am]?.known_aborts);
  check(aborts.every(count => count === 0 || count === 1) && aborts[0] + aborts[1] === 1,
    'runtime_loss_abort_count_unbound');
  const abortAm = managers[aborts.indexOf(1)];
  check(abortAm === recoveryManager && abortAm === writerProof.ingress, 'runtime_loss_recovery_and_writer_share_manager');
  const recovered = await waitFor(snapshot, recoveredSnapshot, {timeout: 240000, code: 'runtime_loss_recovery_timeout'});
  check(managers.every((am, index) => publication[am]?.known_aborts === aborts[index]),
    'runtime_loss_abort_repeated_after_retry');
  const scheduled = managers.map(am => publication[am]?.recovery_scheduled);
  check(scheduled.every(count => count === 0 || count === 1) && scheduled[0] + scheduled[1] === 1,
    'runtime_loss_successful_publication_not_unique');
  const recoveryAm = managers[scheduled.indexOf(1)];
  check(recoveryAm === recoveryManager, 'runtime_loss_recovery_commit_wrong_manager');
  const finalTopology = await adapter.topology();
  check(managers.every(am => {
    const initial = initialTopology.ams?.[am];
    const final = finalTopology.ams?.[am];
    return initial?.ready === true && final?.ready === true && id(initial.pod_uid) && id(initial.instance_id)
      && final.pod_uid === initial.pod_uid && final.instance_id === initial.instance_id;
  }) && hash(finalTopology.ams[writerProof.owner].instance_id) === writerProof.owner_instance_sha256
    && hash(finalTopology.ams[writerProof.ingress].instance_id) === writerProof.ingress_instance_sha256,
  'runtime_loss_manager_process_changed');
  const titleEffects = await adapter.titleEffects({agentId: sibling.agentId, title});
  check(titleEffects?.length === 1 && Number.isSafeInteger(titleEffects[0])
    && titleEffects[0] > before.children[1].last_seq, 'runtime_loss_competing_effect_not_unique');
  for (const am of ['a', 'b']) {
    const current = await read(am, preferencePath);
    check(current?.title === writer.body.title && current.revision === writer.body.revision,
      'runtime_loss_competing_publication_lost');
  }
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
        && retirements[0].kind.runtime_generation === recovered.children[index].runtime_generation
        && (index !== 1 || Number.isSafeInteger(retirements[0].seq) && titleEffects[0] < retirements[0].seq),
      'runtime_loss_generation_retirement_unbound');
    }
  }
  return {previous_pod_sha256: hash(runtime.pod_uid), replacement_pod_sha256: hash(replacement.pod_uid),
    recovered_agents: agents.length, retained_runtime_image: true, retained_home: true,
    retained_conversations: true, known_recovery_aborts: 1, competing_metadata_publications: titleEffects.length,
    recovery_abort_manager: abortAm, recovery_abort_instance_sha256: hash(finalTopology.ams[abortAm].instance_id),
    recovery_commit_manager: recoveryAm, recovery_commit_instance_sha256: hash(finalTopology.ams[recoveryAm].instance_id),
    metadata_writer_manager: writerProof.owner, metadata_writer_instance_sha256: writerProof.owner_instance_sha256,
    metadata_ingress_manager: writerProof.ingress, metadata_ingress_instance_sha256: writerProof.ingress_instance_sha256,
    metadata_request_sha256: hash(writerProof.request_id),
    generation_retirements: agents.length, post_loss_replies: agents.length};
}
