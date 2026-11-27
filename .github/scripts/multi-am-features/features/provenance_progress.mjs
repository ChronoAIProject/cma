import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual as equal} from 'node:util';
import {assertOwner, check, hash, id, other, poll} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

export async function provenanceKnownAbort({adapter, api, record, recordFailure}) {
  const {agentId, sandboxId, operationId} = record;
  check(id(agentId) && id(sandboxId) && id(operationId), 'provenance_progress_lineage_required');
  const owner = assertOwner(await adapter.ownerOf({agentId}), agentId);
  const ingress = other(owner.am);
  const agent = '/api/v1/agents/' + agentId;
  const read = async (am, path) => (await api.rawRequest(am, 'GET', agent + path)).body;
  const patch = (am, revision, system_prompt) => api.rawRequest(am, 'PATCH', agent + '/preferences', {
    key: randomUUID(), body: {expected_revision: revision, system_prompt}, timeout: 8000,
  });
  const control = (action, fields = {}) => adapter.provenanceBackfill({action, agentId, sandboxId, operationId, ...fields});
  const original = await read(owner.am, '/provenance');
  const baseline = await read(owner.am, '/preferences');
  check(original?.agent_id === agentId && original.creation_origin === 'manual'
    && equal(original, await read(ingress, '/provenance')), 'provenance_progress_original_public_readback_invalid');
  check(baseline?.prompt_supported === true && baseline.prompt_pending === false
    && Number.isSafeInteger(baseline.revision), 'provenance_progress_prompt_not_supported');

  const prompt = 'Provenance proof ' + randomUUID();
  let armed, disarmed, digest;
  await withRestoration(async () => {
    armed = await control('arm');
    check(armed?.armed === true && Number.isSafeInteger(armed.count), 'provenance_progress_not_armed');
    const held = await waitForAbortRead(control);
    digest = held.transaction_sha256;
    check(/^[a-f0-9]{64}$/.test(held.commit_transaction_sha256 ?? ''), 'provenance_progress_claim_not_bound');
    const {body: updated} = await api.router.dispatch(agentId, am => {
      check(am === ingress, 'provenance_progress_owner_changed');
      return patch(am, baseline.revision, prompt);
    });
    check(updated?.system_prompt === prompt && updated.revision === baseline.revision + 1
      && updated.prompt_pending === false, 'provenance_progress_peer_write_unbound');
    check((await control('sample'))?.transaction_sha256 === digest,
      'provenance_progress_peer_write_outside_transaction');
    const completed = await poll(() => control('observe'), value => value?.recorded === true
      && value.sweep_finished === true, {timeout: 25000, interval: 250, code: 'provenance_progress_not_recorded_once'});
    check(Number.isSafeInteger(completed.agent_revision) && completed.agent_revision > armed.agent_revision,
      'provenance_progress_peer_revision_lost');
    const failed = await control('verify', {transaction: held.commit_transaction_sha256});
    check(failed?.aborted === true, 'provenance_progress_abort_transaction_unproven');
    for (const am of [owner.am, ingress]) {
      const preferences = await read(am, '/preferences');
      check(preferences?.system_prompt === prompt && preferences.revision === updated.revision,
        'provenance_progress_peer_prompt_lost');
      const derived = await read(am, '/provenance');
      check(derived?.agent_id === agentId && derived.created_at === original.created_at
        && derived.creation_origin === 'legacy_unknown', 'provenance_progress_legacy_readback_invalid');
    }
  }, () => withRestoration(async () => {
    disarmed = await control('restore');
    check(disarmed?.restored === true, 'provenance_progress_restore_failed');
  }, async () => {
    const current = await read(owner.am, '/preferences');
    if (current.system_prompt !== baseline.system_prompt) {
      check(current.system_prompt === prompt && Number.isSafeInteger(current.revision), 'provenance_progress_prompt_restore_foreign_value');
      await patch(ingress, current.revision, baseline.system_prompt);
    }
    for (const am of [owner.am, ingress]) {
      const restored = await read(am, '/preferences');
      check(restored?.system_prompt === baseline.system_prompt && restored.prompt_pending === false,
        'provenance_progress_prompt_restore_failed');
    }
  }, recordFailure), recordFailure);
  check(disarmed.count === armed.count + 1, 'provenance_progress_abort_not_consumed_once');
  for (const am of [owner.am, ingress]) {
    check(equal(await read(am, '/provenance'), original), 'provenance_progress_original_creation_not_restored');
  }
  return {agent_sha256: hash(agentId), ingress, owner: owner.am, owner_epoch: owner.epoch,
    overlapping_transaction_sha256: digest, known_aborts: 1, recorded: 1, original_creation_restored: true};
}

async function waitForAbortRead(control) {
  const timeoutCode = 'provenance_progress_read_not_held';
  let readSeen = false;
  try {
    return await poll(async () => {
      const sample = await control('sample');
      readSeen ||= sample?.matching_read_seen === true;
      return sample;
    }, value => /^[a-f0-9]{64}$/.test(value?.transaction_sha256 ?? ''),
    {timeout: 20000, interval: 150, code: timeoutCode});
  } catch (error) {
    if (error.message !== timeoutCode) throw error;
    if (readSeen) throw new Error('provenance_progress_read_without_claim', {cause: error});
    const fault = await control('abort_fault').catch(() => null);
    if (fault?.entered === true) throw new Error('provenance_progress_fault_entered_without_held_read', {cause: error});
    if (fault?.entered === false) throw new Error('provenance_progress_fault_not_entered', {cause: error});
    throw error;
  }
}

export async function provenanceUnknownCommit({adapter, api, record, recordFailure}) {
  const {agentId, sandboxId, operationId} = record;
  check(id(agentId) && id(sandboxId) && id(operationId), 'provenance_progress_lineage_required');
  const path = '/api/v1/agents/' + agentId + '/provenance';
  const read = async am => (await api.rawRequest(am, 'GET', path)).body;
  const control = (action, args = {}) =>
    adapter.provenanceBackfill({action, agentId, sandboxId, operationId, unknown: true, ...args});
  const original = await read('a');
  check(original?.agent_id === agentId && original.creation_origin === 'manual'
    && equal(await read('b'), original), 'provenance_progress_commit_original_unbound');

  let receipt, transaction;
  await withRestoration(async () => {
    const armed = await control('arm');
    check(armed?.armed === true && armed.configured === true
      && /^[a-f0-9]{64}$/.test(armed.previous_sweep_sha256 ?? ''), 'provenance_progress_commit_not_armed');
    const held = await poll(() => control('sample'), value => /^[a-f0-9]{64}$/.test(value?.commit_transaction_sha256 ?? ''),
      {timeout: 20000, interval: 150, code: 'provenance_progress_commit_target_missing'});
    transaction = held.commit_transaction_sha256;
    check((await control('commit', {transaction}))?.commit_armed === true,
      'provenance_progress_commit_fault_not_armed');
    receipt = await poll(() => control('settle'), value => value?.worker_unknown === true,
      {timeout: 12000, interval: 250, code: 'provenance_progress_commit_worker_outcome_missing'});
    check(receipt.commit_acknowledgements_lost === 5, 'provenance_progress_commit_receipt_unbound');
    const completed = await poll(() => control('observe'), value => value?.recorded === true
      && value.sweep_finished === true, {timeout: 25000, interval: 250, code: 'provenance_progress_commit_not_recovered'});
    check(/^[a-f0-9]{64}$/.test(completed.sweep_sha256 ?? '')
      && completed.sweep_sha256 !== armed.previous_sweep_sha256,
    'provenance_progress_commit_sweep_reused');
    check(completed.new_sweep_receipts === 1, 'provenance_progress_commit_sweep_receipts_unbound');
    for (const am of ['a', 'b']) {
      const derived = await read(am);
      check(derived?.agent_id === agentId && derived.created_at === original.created_at
        && derived.creation_origin === 'legacy_unknown', 'provenance_progress_commit_legacy_readback_invalid');
    }
    const final = await control('observe_commit');
    check(final?.worker_unknown === true && final.commit_acknowledgements_lost === receipt.commit_acknowledgements_lost,
      'provenance_progress_commit_outcome_changed');
  }, async () => {
    const restored = await control('restore');
    check(restored?.restored === true && restored.configured === false, 'provenance_progress_commit_restore_failed');
  }, recordFailure);
  for (const am of ['a', 'b']) {
    check(equal(await read(am), original), 'provenance_progress_commit_original_not_restored');
  }
  return {agent_sha256: hash(agentId), transaction_sha256: transaction,
    commit_acknowledgements_lost: receipt.commit_acknowledgements_lost, worker_commit_outcome: 'unknown',
    new_sweep_receipts: 1, recorded: 1, original_creation_restored: true};
}
