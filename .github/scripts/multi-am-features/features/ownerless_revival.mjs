import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual} from 'node:util';
import {assertOwner, check, hash, id, other, poll} from '../cma/core.mjs';

export async function ownerlessRevival({api, adapter, record, restartOwner, openBoth, complete, pauseObservation,
  waitFor = poll}) {
  const {agentId, sandboxId} = record;
  check(id(agentId) && id(sandboxId), 'revival_lineage_invalid');
  const path = `/api/v1/sandboxes/${sandboxId}`;
  const priorOwner = assertOwner(await adapter.ownerOf({agentId}), agentId);
  const ingress = other(priorOwner.am);
  const readAgent = async am => {
    // Read the requested process directly, including while ownership is absent.
    const {body} = await api.rawRequest(am, 'GET', `/api/v1/agents/${agentId}`);
    check(body?.id === agentId && body.sandbox?.id === sandboxId, 'revival_agent_lineage_changed');
    return body;
  };
  const readEvents = async (am, after) => {
    const {body} = await api.request(am, 'GET', `${path}/events?after=${after}&limit=100`);
    check(Array.isArray(body) && body.length < 100 && body.every((row, index) =>
      row?.sandbox_id === sandboxId && Number.isSafeInteger(row.seq) &&
      row.seq > (index === 0 ? after : body[index - 1].seq)), 'revival_event_page_unbound');
    return body;
  };
  const before = await readAgent(priorOwner.am);
  check(before.status === 'ready' && before.sandbox.status === 'active' &&
    id(before.workspace?.id) && id(before.worktree?.id), 'revival_initial_state_invalid');
  const initialRuntime = await adapter.runtimeFor({agentId});
  check(initialRuntime.agent_id === agentId && initialRuntime.sandbox_id === sandboxId &&
    initialRuntime.ready === true && initialRuntime.pod_uid && initialRuntime.image === record.runtime.image,
  'revival_initial_runtime_unbound');
  const history = await readEvents(priorOwner.am, 0);
  check(history.length > 0, 'revival_event_baseline_missing');
  const after = history.at(-1).seq;
  const admit = async (am, action, key, status) => {
    const receipt = await api.request(am, 'POST', `${path}/${action}`, {key, allowed: [202]});
    check(receipt.status === 202 && receipt.timing?.am === am &&
      receipt.body?.id === sandboxId && receipt.body.status === status &&
      typeof receipt.timing?.request_id === 'string' && receipt.timing.request_id.length > 0,
    'revival_admission_unbound');
    return receipt;
  };
  await admit(ingress, 'pause', randomUUID(), 'pausing');
  if (pauseObservation) await pauseObservation();
  await waitFor(() => readAgent(priorOwner.am), value => value.status === 'paused' &&
    value.sandbox.status === 'paused', {timeout: 240000, code: 'revival_pause_missing'});
  // A paused Agent is still a live resource. Withdraw any retained authority
  // through the real owning process and wait for the durable lease to clear.
  const retainedOwner = await adapter.ownerOf({agentId});
  if (retainedOwner) await restartOwner(assertOwner(retainedOwner, agentId));
  await waitFor(() => adapter.liveOwnerLease({agentId}), value => value === false,
    {timeout: 30000, code: 'revival_ownerless_pause_missing'});
  const topology = await adapter.topology();
  check(['a', 'b'].every(am => topology.ams?.[am]?.ready === true) &&
    topology.ams.a.pod_uid !== topology.ams.b.pod_uid && topology.shared_database === true &&
    topology.coordination_enabled === true, 'revival_two_ready_managers_required');
  check(await adapter.liveOwnerLease({agentId}) === false, 'revival_lease_reappeared_before_admission');
  const key = randomUUID();
  const admitted = await admit(ingress, 'revive', key, 'reviving');
  const replay = await admit(priorOwner.am, 'revive', key, 'reviving');
  check(isDeepStrictEqual(replay.body, admitted.body), 'revival_receipt_changed_on_peer_replay');
  await waitFor(async () => {
    const revived = await readAgent(priorOwner.am);
    check(['paused', 'starting', 'ready'].includes(revived.status) &&
      ['paused', 'reviving', 'active'].includes(revived.sandbox.status), 'revival_terminal_state');
    return revived;
  }, value => value.status === 'ready' && value.sandbox.status === 'active',
  {timeout: 240000, code: 'revival_ready_timeout'});
  const reopened = await openBoth(record);
  check(reopened.both_am_details_and_transcripts === true, 'revival_open_missing');
  for (const am of ['a', 'b']) {
    const current = await readAgent(am);
    check(current.workspace?.id === before.workspace.id && current.worktree?.id === before.worktree.id,
      'revival_hierarchy_changed');
  }
  const owner = assertOwner(await adapter.ownerOf({agentId}), agentId);
  check(owner.epoch > (retainedOwner?.epoch ?? priorOwner.epoch), 'revival_owner_epoch_did_not_advance');
  const runtime = await adapter.runtimeFor({agentId});
  check(runtime.agent_id === agentId && runtime.sandbox_id === sandboxId && runtime.ready === true &&
    runtime.image === initialRuntime.image && runtime.pod_uid && runtime.pod_uid !== initialRuntime.pod_uid,
  'revival_runtime_replacement_unbound');
  await complete(record, other(owner.am));
  const readbacks = await Promise.all(['a', 'b'].map(am => readEvents(am, after)));
  const edges = events => events.filter(row => row.kind?.type === 'revived' ||
    (row.kind?.type === 'status_changed' && row.kind?.from === 'paused' && row.kind?.to === 'reviving'));
  const [first, peer] = readbacks.map(edges);
  check(first.length === 2 && first[0].kind.type === 'status_changed' && first[1].kind.type === 'revived',
    'revival_durable_effect_missing_or_duplicated');
  check(isDeepStrictEqual(peer, first), 'revival_peer_event_history_disagrees');
  return {ingress, replay_ingress: priorOwner.am, prior_owner_epoch: priorOwner.epoch,
    owner_epoch: owner.epoch, durable_admissions: 1, durable_completions: 1,
    retained_owner_restarted: retainedOwner !== null,
    request_sha256: hash(admitted.timing.request_id), replay_request_sha256: hash(replay.timing.request_id)};
}
