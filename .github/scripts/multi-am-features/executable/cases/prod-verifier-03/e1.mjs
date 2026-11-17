import {Blocked} from '../../runner.mjs';
import {acquire, invoke, egRequest, segment, peer, poll, same, digest, retainTurn, delivered,
  assertServingRequest} from './helpers.mjs';

export const reviewedE1Source = '6f03d970cb82adc7d8a1b2346746fc8203422544';
const uuid = value => typeof value === 'string'
  && /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value);
const channelPath = lease => `/v1/channels/${segment(lease.channelId)}`;
const partitionPath = (lease, partition) => `${channelPath(lease)}/partitions/${segment(partition)}`;

export function releaseBody(ctx, preview, terminal, operationId) {
  ctx.assert(uuid(preview.partition_id) && uuid(preview.head) && uuid(operationId)
    && Number.isSafeInteger(preview.version) && preview.version > 0
    && Number.isSafeInteger(preview.epoch) && preview.epoch >= 0
    && preview.phase === 'unknown' && preview.releasable === true, 'e1_exact_releasable_head');
  // Expiry alone is not proof the provider stopped. The actual local provider
  // terminal observation must join this work before asserting the release flags.
  ctx.assert(terminal?.head === preview.head && terminal.terminal === true
    && terminal.inflight === 0 && /^[0-9a-f]{64}$/.test(terminal.evidence_sha256 ?? ''),
  'e1_provider_terminal_proof_required');
  return {partition_id: preview.partition_id, expected_head: preview.head,
    expected_epoch: preview.epoch, expected_version: preview.version, operation_id: operationId,
    provider_terminal_confirmed: true, suppress_delayed_replies: true};
}

export function releaseWinner(ctx, replies, commands) {
  const winners = replies.map((reply, index) => ({...reply, index})).filter(reply => reply.status === 200);
  ctx.assert(winners.length === 1 && replies.filter(reply => reply.status === 409).length === 1,
    'e1_exactly_one_release_winner');
  const winner = winners[0], command = commands[winner.index], result = winner.body;
  ctx.assert(result.operation_id === command.operation_id && result.released_head === command.expected_head
    && result.disposition === 'unknown_abandoned' && result.epoch > command.expected_epoch
    && Number.isSafeInteger(result.released_at_ms), 'e1_release_receipt_exact');
  return {winner: winner.index, command, result};
}

export async function releaseRace(ctx, lease, path, commands) {
  const replicas = ['a', 'b'];
  // An unknown response on one replica must not hand control to cleanup while
  // the other release can still commit. Join both original requests; never retry.
  const outcomes = await Promise.allSettled(replicas.map((replica, index) => egRequest(ctx, lease,
    {method: 'POST', path, actor: 'release_operator', replica, body: commands[index],
      allowed: [200, 409], returnResponse: true})));
  const failures = outcomes.filter(outcome => outcome.status === 'rejected').map(outcome => outcome.reason);
  if (failures.length) {
    const error = new AggregateError(failures, 'e1_release_race_incomplete');
    // The shared runner retains this privately with the primary failure. A
    // sibling's acknowledged result remains available for exact reconciliation.
    error.response = {releases: outcomes.map((outcome, index) => ({replica: replicas[index],
      outcome: outcome.status, ...(outcome.status === 'fulfilled' ? {response: outcome.value} : {})}))};
    throw error;
  }
  return outcomes.map(outcome => outcome.value);
}

export function assertReleasedState(ctx, before, after, journal, audit, command, result, expectedActor) {
  ctx.assert(before.queue[0] === command.expected_head && same(after.queue, before.queue.slice(1)),
    'e1_release_advances_one_head');
  ctx.assert(journal.phase?.state === 'unknown_abandoned'
    && journal.phase.operation_id === command.operation_id && journal.tombstone === true
    && journal.request_hash === before.request_hash && journal.body_sha256 === before.body_sha256,
  'e1_release_preserves_request_and_tombstone');
  ctx.assert(audit.kind === 'cmaeg_unknown_abandoned_v1' && same(audit.request, command)
    && same(audit.result, result) && audit.original_phase?.state === 'unknown'
    && audit.request_hash === before.request_hash && audit.actor_subject_sha256 === expectedActor,
  'e1_release_authenticated_audit');
}

export async function pendingReply(ctx, lease, request) {
  for (;;) {
    const remaining = ctx.remainingMs?.() ?? 30000;
    ctx.assert(!ctx.signal?.aborted && remaining > 0, 'e1_reply_poll_budget');
    const response = await egRequest(ctx, lease, {...request, timeoutMs: Math.min(remaining, 30000),
      allowed: [200, 409], returnResponse: true});
    if (response.status === 200) {
      ctx.assert(response.body?.delivery?.outcome === 'confirmed', 'e1_delayed_delivery_confirmed');
      return response.body;
    }
    ctx.assert(response.body?.detail === 'routed_reply_in_progress'
      && response.body.retry_after_seconds === 2 && response.retryAfterSeconds === 2,
    'e1_only_pending_reply_is_pollable');
    ctx.assert(!ctx.signal?.aborted && (ctx.remainingMs?.() ?? 3000) > 2000, 'e1_reply_poll_budget');
    // The contract explicitly permits polling only this same key/body/capability.
    await new Promise(resolve => setTimeout(resolve, 2000));
  }
}

// Supplemental E1 coverage stays outside the frozen C275/API denominators.
// The sole parent invokes this definition with executeCase and its own row.
export const e1Journey = {
  id: 'E1:EG-2171', scope: 'eg', fixtures: ['eg'],
  steps: ['step_1', 'step_2', 'step_3', 'step_4', 'expected_result'],
  async run(ctx) {
    let lease, parent, successor, unknown, target, delayed, saved, frozen, terminal, preview, released;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'e1_unknown_head');
      for (const method of ['armAcceptanceLoss', 'platformSend', 'readJournal', 'readPartition',
        'providerTerminal', 'releaseAudit', 'platformDeliveries', 'resumeSuccessor', 'waitForTurn',
        'waitForDelivery', 'drain']) {
        if (typeof lease[method] !== 'function') throw new Blocked('e1_fixture_' + method.toLowerCase());
      }
      if (lease.releaseEnabled !== true || lease.reviewedSource !== reviewedE1Source
        || !lease.replicas.every(replica => replica.source === reviewedE1Source)) {
        throw new Blocked('e1_reviewed_local_release_fixture_required');
      }
      for (const field of ['channelId', 'conversation', 'updateBody', 'keys', 'expected']) {
        if (lease[field] == null) throw new Blocked('e1_fixture_field_' + field.toLowerCase());
      }
      ctx.assert(/^[0-9a-f]{64}$/.test(lease.expected.releaseActorSha256 ?? '')
        && lease.keys.releaseA !== lease.keys.releaseB,
        'e1_release_fixture_identity');
      await invoke(lease, 'armAcceptanceLoss', {conversation: lease.conversation, holdSuccessorAcceptance: true});
      parent = await invoke(lease, 'platformSend', {replica: ctx.am, conversation: lease.conversation,
        text: 'synthetic unresolved parent', variant: 'accepted_response_lost'});
      ctx.assert(parent.eventId && parent.responseId && uuid(parent.workId) && uuid(parent.partitionId),
        'e1_original_admission_identity');
      unknown = await poll(ctx, () => invoke(lease, 'readJournal', {workId: parent.workId}),
        value => value.phase?.state === 'unknown', 'e1_unknown_parent_timeout');
      ctx.assert(unknown.dispatch_count === 1 && unknown.accepted_at === null
        && unknown.request_hash && unknown.body_sha256, 'e1_unknown_does_not_claim_acceptance');
      ctx.assert(unknown.amRequests?.length > 0, 'e1_actual_am_requests_required');
      for (const receipt of unknown.amRequests) assertServingRequest(ctx, receipt);
      ctx.assert(unknown.amRequests.some(receipt => receipt.owner_proof?.kind === 'non_owner'
        && receipt.owner_proof.owner !== receipt.am && receipt.owner_proof.agent_sha256 === digest(parent.agentId)
        && receipt.owner_proof.request_id === receipt.request_id), 'e1_actual_non_owner_dispatch');
      await ctx.observe('e1_parent_dispatch', async () => ({schema: 'multi-am-eg-evidence-v1', secrets_redacted: true,
        event_sha256: digest(parent.eventId), response_sha256: digest(parent.responseId),
        requests: unknown.amRequests, eg_receipts: []}));
      if (!Number.isSafeInteger(parent.expiresAt) || parent.expiresAt * 1000 - Date.now() > 45000)
        throw new Blocked('e1_bounded_authentic_event_expiry_required');
      target = await egRequest(ctx, lease, {method: 'POST', actor: 'agent',
        path: `/v1/events/${segment(parent.eventRef)}/reply-target`, body: {}});
      ctx.assert(target.reply_target_ref && target.expires_at > Date.now() / 1000, 'e1_live_reply_capability');
      successor = await invoke(lease, 'platformSend', {replica: peer(ctx.am), conversation: lease.conversation,
        text: 'synthetic FIFO successor', variant: 'held_successor'});
      frozen = await invoke(lease, 'readPartition', {partitionId: parent.partitionId});
      ctx.assert(same(frozen.queue, [parent.workId, successor.workId]) && frozen.successor_dispatch_count === 0,
        'e1_unrelated_successor_held');
    });
    await ctx.step('step_2', async () => {
      const request = {method: 'POST', actor: 'agent', replica: peer(ctx.am),
        path: `/v1/reply-targets/${segment(target.reply_target_ref)}/messages`,
        body: {text: 'synthetic delayed answer'}, key: lease.keys.reply};
      delayed = await pendingReply(ctx, lease, request);
      const replay = await egRequest(ctx, lease, {...request, replica: ctx.am});
      ctx.assert(same(replay, delayed), 'e1_delayed_poll_same_receipt');
      const changed = await egRequest(ctx, lease, {...request, body: {text: 'changed answer'}, allowed: [409]});
      ctx.assert(changed.detail === 'reply_idempotency_conflict', 'e1_delayed_changed_body_refused');
      const deliveries = await invoke(lease, 'platformDeliveries', {key: lease.keys.reply});
      ctx.assert(deliveries.length === 1 && deliveries[0].contentDigest === digest(request.body)
        && deliveries[0].destination === parent.destination, 'e1_delayed_answer_delivered_once');
      const current = await invoke(lease, 'readJournal', {workId: parent.workId});
      const queue = await invoke(lease, 'readPartition', {partitionId: parent.partitionId});
      ctx.assert(current.phase?.state === 'unknown' && same(queue.queue, frozen.queue)
        && queue.successor_dispatch_count === 0, 'e1_delayed_reply_does_not_release_fifo');
    });
    await ctx.step('step_3', async () => {
      const before = await egRequest(ctx, lease, {path: channelPath(lease)});
      const changedBody = {...lease.updateBody, name: `${before.definition.name} changed`,
        expected_version: before.version, reply: {...lease.updateBody.reply,
          ack_after_ms: (before.definition.reply.ack_after_ms + 100) % 60001}};
      saved = await egRequest(ctx, lease, {method: 'PUT', path: channelPath(lease), body: changedBody,
        replica: peer(ctx.am)});
      ctx.assert(saved.version > before.version && saved.record_id === before.record_id
        && saved.definition.name === changedBody.name, 'e1_definition_edit_saved');
      const current = await egRequest(ctx, lease, {path: channelPath(lease)});
      const original = await invoke(lease, 'readJournal', {workId: parent.workId});
      ctx.assert(current.version === saved.version
        && current.definition.reply.ack_after_ms === changedBody.reply.ack_after_ms
        && original.request_hash === unknown.request_hash && original.body_sha256 === unknown.body_sha256
        && original.dispatch_count === 1, 'e1_definition_edit_preserves_unknown_request');
      terminal = await invoke(lease, 'providerTerminal', {workId: parent.workId});
      preview = await poll(ctx, () => egRequest(ctx, lease, {
        path: partitionPath(lease, parent.partitionId) + '/head'}), value => value.releasable === true,
      'e1_release_expiry_timeout');
      ctx.assert(preview.head === parent.workId && preview.phase === 'unknown', 'e1_unknown_head_still_exact');
    });
    await ctx.step('step_4', async () => {
      const commands = [lease.keys.releaseA, lease.keys.releaseB].map(key => releaseBody(ctx, preview, terminal, key));
      const path = partitionPath(lease, parent.partitionId) + '/release';
      for (const actor of ['owner_without_release_grant', 'outsider']) {
        const denied = await egRequest(ctx, lease, {method: 'POST', path, body: commands[0], actor,
          replica: peer(ctx.am), allowed: [403, 404], returnResponse: true});
        ctx.assert([403, 404].includes(denied.status), 'e1_release_explicit_authority_required');
      }
      const stale = await egRequest(ctx, lease, {method: 'POST', path, actor: 'release_operator',
        body: {...commands[0], expected_version: preview.version + 1}, allowed: [409], returnResponse: true});
      ctx.assert(stale.status === 409, 'e1_stale_release_refused');
      const responses = await releaseRace(ctx, lease, path, commands);
      released = releaseWinner(ctx, responses, commands);
      const after = await invoke(lease, 'readPartition', {partitionId: parent.partitionId});
      const journal = await invoke(lease, 'readJournal', {workId: parent.workId});
      const audit = await invoke(lease, 'releaseAudit', {partitionId: parent.partitionId, workId: parent.workId});
      assertReleasedState(ctx, {...frozen, request_hash: unknown.request_hash, body_sha256: unknown.body_sha256},
        after, journal, audit, released.command, released.result, lease.expected.releaseActorSha256);
      const duplicate = await egRequest(ctx, lease, {method: 'POST', path, actor: 'release_operator',
        replica: responses[released.winner].eg_receipt.replica === 'a' ? 'b' : 'a', body: released.command});
      ctx.assert(same(duplicate, released.result), 'e1_release_duplicate_original_receipt');
      const still = await invoke(lease, 'readPartition', {partitionId: parent.partitionId});
      ctx.assert(same(still.queue, after.queue) && still.successor_dispatch_count <= 1,
        'e1_release_duplicate_cannot_pop_successor');
      await ctx.observe('e1_release_disposition', async () => ({operation_sha256: digest(released.command.operation_id),
        head_sha256: digest(released.result.released_head), disposition: released.result.disposition,
        audit_sha256: digest(audit), preserved_uncertainty: true}));
    });
    await ctx.step('expected_result', async () => {
      const stale = await egRequest(ctx, lease, {method: 'POST', actor: 'agent', replica: peer(ctx.am),
        path: `/v1/reply-targets/${segment(target.reply_target_ref)}/messages`, body: {text: 'stale late answer'},
        key: lease.keys.staleReply, allowed: [403, 410], returnResponse: true});
      ctx.assert([403, 410].includes(stale.status), 'e1_stale_reply_refused');
      await invoke(lease, 'resumeSuccessor', {eventId: successor.eventId});
      const turn = await invoke(lease, 'waitForTurn', {eventId: successor.eventId, timeoutMs: ctx.remainingMs()});
      await retainTurn(ctx, successor, turn);
      const receipt = await invoke(lease, 'waitForDelivery', {eventId: successor.eventId, timeoutMs: ctx.remainingMs()});
      delivered(ctx, successor, receipt);
      const final = await invoke(lease, 'drain', {timeoutMs: ctx.remainingMs()});
      ctx.assert(final.pending === 0 && final.unknown === 0 && final.abandoned === 1
        && final.parentDispatches === 1 && final.successorDispatches === 1
        && final.delayedDeliveries === 1 && final.staleDeliveries === 0, 'e1_final_no_replay_or_stale_delivery');
      await ctx.observe('e1_final_counts', async () => final);
    });
  },
};
