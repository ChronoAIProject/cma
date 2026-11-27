import {randomUUID} from 'node:crypto';
import {assertOwner, check, hash, id, other, poll} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';
import {isDeepStrictEqual as equal} from 'node:util';

export async function preparedSchemaRefusal({adapter, admit, cleanup, recordFailure, barrier = 'worktree_schema'}) {
  const ingress = 'a';
  const reservations = await poll(() => adapter.preparedReady(),
    rows => Array.isArray(rows) && rows.length > 0, {timeout: 180000, code: 'no_ready_prepared_reservation'});
  const runtimeBefore = new Map(await Promise.all(reservations.map(async sandboxId =>
    [sandboxId, await adapter.preparedRuntimeEvidence({sandboxId})])));
  let record, changed, result;
  const race = action => adapter.preparedSchemaRace({action, operationId: record.operationId, reservations, barrier});
  await withRestoration(async () => {
    await adapter.preparedSchemaBarrier({arm: true});
    record = await admit(ingress, false, {});
    changed = await poll(() => race('inject'), value => value !== null,
      {timeout: 120000, interval: 100, code: 'prepared_worktree_transaction_not_observed'});
    check(id(changed.agentId) && reservations.includes(changed.sandboxId), 'prepared_schema_race_lineage_unbound');
    record.agentId = changed.agentId;
    record.sandboxId = changed.sandboxId;
    await adapter.preparedSchemaBarrier({arm: false});
    const owner = assertOwner(await adapter.ownerOf({agentId: record.agentId}), record.agentId);
    const refusals = await poll(() => adapter.preparedSchemaRefusals({agentId: record.agentId, barrier}),
      value => value?.[owner.am] >= 1, {timeout: 25000, code: 'prepared_schema_race_refusal_missing'});
    check(refusals[other(owner.am)] === 0, 'prepared_schema_race_refusal_wrong_manager');
    const diagnostics = await adapter.publicationEvidence({agentId: record.agentId});
    const aborts = am => diagnostics[am]?.known_aborted_cas?.publish_connection ?? 0;
    check(aborts(owner.am) === 1 && aborts(other(owner.am)) === 0,
      'prepared_schema_race_abort_not_attributed');
    const state = await race('read');
    const changesSchema = barrier === 'worktree_schema';
    const addsCleanup = barrier === 'worktree_cleanup';
    const changesLease = barrier === 'work_lease';
    check(state.agentId === record.agentId && state.sandboxId === record.sandboxId
      && (changesSchema ? state.schema === 999 : state.schema === changed.schema)
      && state.cleanup_marked === addsCleanup && state.work_lease_changed === changesLease
      && state.receipt_count === changed.receipt_count
      && state.conversation_unpublished === true && state.ready_fences_absent === true,
    'prepared_schema_race_partial_publication');
    const runtime = await adapter.preparedRuntimeEvidence({sandboxId: record.sandboxId,
      operationId: record.operationId});
    check(runtime?.receipts?.length === 3 && equal(runtime, runtimeBefore.get(record.sandboxId)),
      'prepared_refusal_runtime_mutation_receipts_changed');
    const finalOwner = assertOwner(await adapter.ownerOf({agentId: record.agentId}), record.agentId);
    check(owner.am === finalOwner.am && owner.epoch === finalOwner.epoch && owner.pod_uid === finalOwner.pod_uid,
      'prepared_schema_race_owner_changed');
    result = {ingress, owner: owner.am, owner_epoch: owner.epoch, agent_sha256: hash(record.agentId),
      observed_known_abort: true, prepared_schema_refused: changesSchema,
      prepared_cleanup_refused: addsCleanup, prepared_work_lease_refused: changesLease,
      startup_receipt_count: state.receipt_count, runtime_receipt_count: runtime.receipts.length,
      runtime_receipt_format: runtime.format};
  }, async () => {
    await withRestoration(async () => {
      if (record) {
        const restored = await race('restore');
        check(restored.restored === 1 || (!changed && restored.restored === 0),
          'prepared_schema_race_restore_unproven');
      }
    }, () => adapter.preparedSchemaBarrier({arm: false}), recordFailure);
  }, recordFailure);
  await cleanup(record);
  return result;
}


export async function preparedParentPauseRefusal({api, adapter, admit, cleanup, recordFailure}) {
  const reservations = await poll(() => adapter.preparedReady(),
    rows => Array.isArray(rows) && rows.length > 0,
    {timeout: 180000, code: 'no_ready_prepared_reservation'});
  const ingress = 'a';
  let record, owner, receipt, held;
  const race = action => adapter.preparedSchemaRace({
    action, operationId: record.operationId, reservations, barrier: 'parent_pause',
  });
  await withRestoration(async () => {
    await adapter.preparedSchemaBarrier({arm: true, parentPause: true});
    record = await admit(ingress, false, {});
    held = await poll(() => race('sample'), value => value !== null,
      {timeout: 120000, interval: 100, code: 'prepared_parent_pause_transaction_missing'});
    check(id(held.agentId) && reservations.includes(held.sandboxId)
      && /^[a-f0-9]{64}$/.test(held.transactionSha256) && held.sandboxStatus === 'active'
      && held.state === 'starting' && held.step === 'open_conversation' && held.receipt_count === 2
      && held.conversation_unpublished && held.ready_fences_absent,
    'prepared_parent_pause_precondition_invalid');
    Object.assign(record, {agentId: held.agentId, sandboxId: held.sandboxId});
    owner = assertOwner(await adapter.ownerOf({agentId: record.agentId}), record.agentId);
    const pauseIngress = other(owner.am);
    receipt = await api.request(pauseIngress, 'POST', `/api/v1/sandboxes/${record.sandboxId}/pause`,
      {key: randomUUID(), direct: true, allowed: [202]});
    check(receipt.status === 202 && receipt.body?.id === record.sandboxId && receipt.body.status === 'pausing'
      && receipt.timing?.am === pauseIngress && id(receipt.timing.request_id),
    'prepared_parent_pause_admission_unbound');
    const stillHeld = await race('sample');
    check(stillHeld?.transactionSha256 === held.transactionSha256
      && ['pausing', 'paused'].includes(stillHeld.sandboxStatus),
    'prepared_parent_pause_order_unproven');
  }, () => adapter.preparedSchemaBarrier({arm: false}), recordFailure);
  const {agentId, sandboxId, operationId} = record;
  const diagnostics = await poll(() => adapter.publicationEvidence({agentId}),
    value => value?.[owner.am]?.known_aborted_cas?.publish_connection >= 1,
    {timeout: 25000, code: 'prepared_parent_pause_abort_missing'});
  check(diagnostics[owner.am].known_aborted_cas.publish_connection === 1
    && ['a', 'b'].every(am => !diagnostics[am]?.prepared_commit_work_lease_sha256?.length)
    && !diagnostics[other(owner.am)]?.known_aborted_cas?.publish_connection,
  'prepared_parent_pause_publication_unproven');
  const state = await race('read');
  check(state?.agentId === agentId && state.sandboxId === sandboxId
    && ['pausing', 'paused'].includes(state.sandboxStatus) && state.state !== 'ready'
    && state.receipt_count === held.receipt_count && state.conversation_unpublished && state.ready_fences_absent,
  'prepared_parent_pause_partial_publication');
  await cleanup(record);
  return {ingress: receipt.timing.am, owner: owner.am, owner_epoch: owner.epoch,
    agent_sha256: hash(agentId), prepared_reservation_sha256: hash(sandboxId),
    lifecycle_operation_sha256: hash(operationId), request_sha256: {pause: hash(receipt.timing.request_id)},
    observed_known_abort: true, prepared_parent_pause_refused: true, startup_receipt_count: state.receipt_count};
}
