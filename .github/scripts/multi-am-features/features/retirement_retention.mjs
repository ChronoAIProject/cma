import {randomUUID} from 'node:crypto';
import {assertOracle, assertOwner, assertResponse, check, hash, id, other, poll, terminal} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

export async function retirementRetention({adapter, user, outsider, record, cleanup, admissions, waitForPurge = poll}) {
  const {agentId, sandboxId} = record;
  check(id(agentId) && id(sandboxId), 'retirement_lineage_required');
  const agentPath = `/api/v1/agents/${agentId}`;
  const sandboxPath = `/api/v1/sandboxes/${sandboxId}`;
  const snapshot = () => adapter.lifecycleSnapshot({sandboxId, agentIds: [agentId]});
  const before = await snapshot();
  check(before?.status === 'active' && before.children?.length === 1 &&
    before.children[0]?.status === 'ready' && Number.isSafeInteger(before.children[0].last_seq) &&
    before.children[0].last_seq > 0, 'retirement_live_history_unbound');
  for (const am of ['a', 'b']) {
    await outsider.request(am, 'DELETE', sandboxPath, {key: randomUUID(), allowed: [404]});
    await outsider.request(am, 'DELETE', agentPath, {key: randomUUID(), allowed: [404]});
  }
  const protectedState = await snapshot();
  check(protectedState?.status === 'active' && protectedState.children?.[0]?.status === 'ready' &&
    protectedState.children[0].last_seq >= before.children[0].last_seq, 'retirement_foreign_delete_changed_state');
  const owner = assertOwner(await adapter.ownerOf({agentId}), agentId);
  const ingress = other(owner.am);
  const stopPath = agentPath + '/stop', stop = {body: {}, key: randomUUID(), allowed: [202]};
  await user.request(ingress, 'POST', stopPath, stop);
  const stopped = await poll(async () => (await user.request(owner.am, 'GET', agentPath)).body, value => {
    check(value?.id === agentId && value.sandbox?.id === sandboxId, 'stop_readback_wrong_lineage');
    return value.status === 'stopped';
  }, {timeout: 90000, code: 'stop_did_not_reach_terminal_state'});
  check(stopped.stop_reason === 'owner', 'stop_reason_not_owner');
  const disposition = await snapshot();
  check(disposition?.status === 'active' && disposition.children?.[0]?.status === 'stopped'
    && disposition.children[0].last_seq > protectedState.children[0].last_seq, 'stop_terminal_snapshot_unbound');
  await user.request(owner.am, 'POST', stopPath, stop);
  const changed = await user.request(ingress, 'POST', stopPath, {...stop, body: {reason: 'owner'}, allowed: [422]});
  check(changed.body?.code === 'idempotency_key_reused', 'stop_changed_request_refusal_untyped');
  const stoppedEvents = [];
  for (const am of [ingress, owner.am]) {
    const detail = (await user.request(am, 'GET', agentPath)).body;
    check(detail?.id === agentId && detail.sandbox?.id === sandboxId
      && detail.status === 'stopped' && detail.stop_reason === stopped.stop_reason, 'stop_peer_readback_changed');
    const {body: log} = await user.request(am, 'GET', agentPath + '/events?after=' + protectedState.children[0].last_seq);
    check(Array.isArray(log?.items) && log.next === undefined, 'stop_event_tail_unbound');
    const matches = log.items.filter(entry => entry.kind?.type === 'stopped' && entry.kind.reason === 'owner');
    check(matches.length === 1 && Number.isSafeInteger(matches[0].seq), 'stop_event_missing_or_duplicated');
    stoppedEvents.push(matches[0].seq);
  }
  check(stoppedEvents[0] === stoppedEvents[1], 'stop_event_peer_mismatch');
  const replayed = await snapshot();
  check(replayed?.children?.[0]?.revision === disposition.children[0].revision
    && replayed.children[0].last_seq === disposition.children[0].last_seq, 'stop_replay_mutated_terminal_agent');
  const cleanupProof = await cleanup(record);
  const {deleteKey, deleteReceipt} = cleanupProof ?? {};
  const deleteIngress = deleteReceipt?.timing?.am;
  check(id(deleteKey) && ['a', 'b'].includes(deleteIngress) && deleteReceipt.status === 202,
    'delete_original_receipt_unbound');
  const retained = await snapshot();
  check(retained?.status === 'retired' && retained.runtime_detached === true && retained.purge_deadline_future === true &&
    retained.children?.length === 1 && retained.children[0]?.status === 'deleted' &&
    retained.children[0].last_seq > before.children[0].last_seq, 'retirement_retained_state_or_deadline_missing');
  const deleteReplay = await user.request(other(deleteIngress), 'DELETE', agentPath,
    {key: deleteKey, allowed: [202]});
  check(deleteReplay.status === deleteReceipt.status && deleteReplay.body === deleteReceipt.body,
    'delete_peer_replay_changed_receipt');
  const events = [], deletedEvents = [];
  for (const am of ['a', 'b']) {
    const [agent, sandbox, history, agentHistory] = await Promise.all([
      user.request(am, 'GET', agentPath), user.request(am, 'GET', sandboxPath),
      user.request(am, 'GET', `${sandboxPath}/events?limit=200`),
      user.request(am, 'GET', agentPath + '/events?after=' + stoppedEvents[0]),
    ]);
    check(agent.body?.id === agentId && agent.body.status === 'deleted' && sandbox.body?.id === sandboxId &&
      sandbox.body.status === 'retired', 'retirement_exact_public_readback_missing');
    check(Array.isArray(history.body) && history.body.length < 200 &&
      history.body.every(entry => entry.sandbox_id === sandboxId && Number.isSafeInteger(entry.seq)),
    'retirement_history_unbound');
    const retired = history.body.filter(entry => entry.kind?.type === 'status_changed' && entry.kind?.to === 'retired');
    check(retired.length === 1, 'retirement_history_missing_or_duplicated');
    events.push(retired[0].seq);
    check(Array.isArray(agentHistory.body?.items) && agentHistory.body.next === undefined,
      'delete_event_tail_unbound');
    const deleted = agentHistory.body.items.filter(entry => entry.kind?.type === 'deleted');
    check(deleted.length === 1 && Number.isSafeInteger(deleted[0].seq), 'delete_event_missing_or_duplicated');
    deletedEvents.push(deleted[0].seq);
  }
  check(events[0] === events[1], 'retirement_peer_history_mismatch');
  check(deletedEvents[0] === deletedEvents[1], 'delete_event_peer_mismatch');
  const terminal = await snapshot();
  check(terminal?.revision === retained.revision && terminal.children?.[0]?.revision === retained.children[0].revision
    && terminal.children[0].last_seq === retained.children[0].last_seq, 'delete_replay_mutated_terminal_lineage');
  const title = await adapter.titleIntent({agentId});
  check(title?.accepted === true && title.request_sha256 === record.generatedTitleProof?.digest
    && title.turn_sequence === record.generatedTitleProof?.turnSequence,
  'retirement_title_custody_changed');
  const purged = await observeEligiblePurge({adapter, user, record, admissions, paths: [agentPath, sandboxPath], waitForPurge});
  return {agent_sha256: hash(agentId), sandbox_sha256: hash(sandboxId), terminal_event_count: 1,
    stop_ingress: ingress, stop_owner: owner.am, stop_owner_epoch: owner.epoch, stop_events: 1, delete_events: 1,
    retained_history: true, observed_future_purge_deadline: true, retained_title_custody: true, ...purged};
}

export async function unresolvedTitleRetirement({adapter, api, create, cleanup, recordFailure}) {
  const record = await create('a', false, {title_mode: 'automatic'});
  const {agentId, sandboxId, operationId} = record;
  const caseId = 'retirement-title-' + randomUUID();
  const marker = 'WS4_TITLE_' + randomUUID().replaceAll('-', '');
  let stream, titleDigest;
  await adapter.prepareTurn({caseId, marker, title: 'lost_reply'});
  await withRestoration(async () => {
    await adapter.releaseTurn({caseId});
    stream = await api.send('a', agentId, 'Return the exact marker ' + marker + ' without making changes.', caseId);
    const input = await poll(() => adapter.oracle({caseId}), observed => observed.title_requests === 1,
      {timeout: 30000, interval: 100, code: 'retirement_title_request_missing'});
    titleDigest = input.title_request_sha256;
    check(/^[a-f0-9]{64}$/.test(titleDigest ?? '') && input.title_responses === 0, 'retirement_title_request_unbound');
    await adapter.releaseTurn({caseId, title: true});
    await poll(() => adapter.oracle({caseId}), observed => observed.title_lost_replies === 1,
      {timeout: 10000, interval: 100, code: 'retirement_title_reply_loss_missing'});
    const response = await poll(async () => assertResponse((await api.rawRequest('b', 'GET',
      '/api/v2/responses/' + stream.responseId)).body, agentId, stream.responseId), terminal,
    {timeout: 30000, code: 'retirement_title_turn_did_not_settle'});
    check(response.status === 'completed', 'retirement_title_turn_failed');
    const custody = await adapter.titleIntent({agentId});
    check(custody?.request_sha256 === titleDigest && custody.accepted === false
      && Number.isSafeInteger(custody.turn_sequence), 'retirement_unresolved_title_custody_missing');
  }, async () => {
    try { await adapter.releaseTurn({caseId, title: true}); }
    finally { if (stream) { stream.abort(); await stream.done; } }
  }, recordFailure);

  await cleanup(record);
  const admissions = [{agentId, operationId}];
  const paths = ['/api/v1/agents/' + agentId, '/api/v1/sandboxes/' + sandboxId];
  const purged = await observeEligiblePurge({adapter, user: api, record, admissions, paths, titleDigest,
    waitForPurge: poll, afterArm: async (target, armed) => {
      const match = observations => ['a', 'b'].flatMap(am => {
        const sample = observations?.[am];
        return sample?.title_deferrals >= 1 && sample.refused_purge_attempts >= 1
          ? sample.refused_lease_digests ?? [] : [];
      });
      const deferrals = await poll(() => adapter.retirementTitleDeferral({agentId, sandboxId}),
        observations => match(observations).length > 0,
        {timeout: 30000, code: 'retirement_live_title_did_not_defer_purge'});
      const digests = [...new Set(match(deferrals))];
      check(digests.length === 1 && /^[a-f0-9]{64}$/.test(digests[0]), 'retirement_refused_lease_not_bound');
      const held = await adapter.retirementPurge(target);
      check(held.present === true && held.agents === 1 && held.unresolved_title_active === true
        && ['agent_events', 'sandbox_events', 'workspaces', 'worktrees'].every(key => held[key] === true)
        && held.creation_sha256 === armed.creation_sha256, 'retirement_live_title_retention_unproven');
      const advanced = await adapter.retirementPurge({...target, advanceTitle: true, refusedLeaseSha256: digests[0]});
      check(advanced.title_advanced === true && advanced.creation_sha256 === armed.creation_sha256,
        'retirement_title_window_not_advanced');
    }});
  const observed = await adapter.oracle({caseId});
  assertOracle(observed, caseId, 1);
  check(observed.title_requests === 1 && observed.title_lost_replies === 1 && observed.title_responses === 0
    && observed.title_request_sha256 === titleDigest, 'retirement_title_effect_replayed');
  return {agent_sha256: hash(agentId), sandbox_sha256: hash(sandboxId),
    unresolved_title_deferred_purge: true, title_request_count: observed.title_requests, ...purged};
}

async function observeEligiblePurge({adapter, user, record, admissions, paths, waitForPurge, titleDigest, afterArm}) {
  const {agentId, sandboxId, operationId} = record;
  check(id(operationId), 'retirement_purge_lifecycle_required');
  const target = {agentId, sandboxId, operationId, admissions, ...(titleDigest ? {titleDigest} : {})};
  const armed = await adapter.retirementPurge({...target, arm: true});
  check(armed?.armed === true && armed.agents === admissions.length
    && /^[a-f0-9]{64}$/.test(armed.creation_sha256 ?? ''), 'retirement_purge_not_armed');
  await afterArm?.(target, armed);
  const fields = ['agent_events', 'sandbox_events', 'workspaces', 'worktrees'];
  let samples = 0;
  await waitForPurge(() => adapter.retirementPurge(target), sample => {
    check(typeof sample?.present === 'boolean' && sample.agents === (sample.present ? armed.agents : 0)
      && fields.every(key => sample[key] === sample.present), 'retirement_purge_partially_removed_lineage');
    check(sample.creation_sha256 === armed.creation_sha256, 'retirement_purge_changed_creation_provenance');
    samples += 1;
    return !sample.present;
  }, {timeout: 90000, code: 'retirement_purge_did_not_finish'});
  for (const am of ['a', 'b']) {
    for (const path of paths) {
      const response = await user.rawRequest(am, 'GET', path, {allowed: [404]});
      check(response.status === 404, 'retirement_purge_public_readback_present');
    }
  }
  return {eligible_purge: true, purged_agents: armed.agents, purge_snapshot_samples: samples,
    retained_creation_provenance: true};
}
