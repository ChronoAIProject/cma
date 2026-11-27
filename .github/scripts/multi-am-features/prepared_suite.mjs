import {readFile, writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {isDeepStrictEqual as equal} from 'node:util';
import {assertOwner, check, hash, poll} from './cma/core.mjs';
import {runJourney} from './cma/journey.mjs';
import {readConfig} from './config.mjs';
import {journeyFailureSummary, retainFailure, withRestoration} from './failure.mjs';
import {openPreparedBrowser} from './prepared_browser.mjs';
import {preparedSchemaRefusal} from './features/prepared_schema_refusal.mjs';
import {writerCoverage} from './writer_coverage.mjs';

const isDigest = value => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);
const OLD_WORK_LEASE_MS = 18 * 60 * 1000;

async function servingManagers(adapter) {
  const topology = await adapter.topology();
  const managers = ['a', 'b'].map(am => topology.ams?.[am]);
  check(topology.shared_database === true && topology.coordination_enabled === true
    && managers.every(member => member?.ready === true && typeof member.instance_id === 'string'
      && member.instance_id && typeof member.pod_uid === 'string' && member.pod_uid)
    && managers[0].instance_id !== managers[1].instance_id && managers[0].pod_uid !== managers[1].pod_uid,
  'prepared_browser_ingress_unbound');
  return topology.ams;
}

export async function preparedJourney({adapter, create, openBoth, cleanup, step, browser, recordFailure,
  recordAbortEvidence = () => {}}) {
  const evidenceFor = record => adapter.preparedEvidence({
    agentId: record.agentId, sandboxId: record.sandboxId, operationId: record.operationId,
  });
  const consumed = new Set();
  for (const ingress of ['a', 'b']) {
    await step('cold_comparison_on_' + ingress, async () => {
      const record = await create(ingress, false, {profile: {system_prompt: 'CI cold comparison with custom instructions'}});
      const observed = await evidenceFor(record);
      check(observed.agent_ready === true && observed.consumed === false,
        'cold_comparison_consumed_prepared_reservation');
      await cleanup(record);
      return {ingress, cold_admission_confirmed: true};
    });
    await step('prepared_create_on_' + ingress, () => browser.forIngress(ingress, async page => {
      const ready = await poll(() => adapter.preparedReady(),
        rows => Array.isArray(rows) && rows.length > 0,
        {timeout: 180000, code: 'no_ready_prepared_reservation'});
      const initialManagers = await servingManagers(adapter);
      const ingressMember = initialManagers[ingress];
      const nativeBefore = new Map(await Promise.all(ready.map(async sandboxId =>
        [sandboxId, await adapter.preparedRuntimeEvidence({sandboxId})])));
      let opening, record, collision;
      const createAndOpen = onAdmission => create(ingress, false, {}, ({agentId}) => {
        opening = page.openStarting(agentId).then(started => ({started}), error => ({error}));
      }, onAdmission);
      if (ingress === 'b') {
        let admission, pending;
        const race = action => adapter.preparedSchemaRace({
          action, operationId: admission.operationId, reservations: ready, barrier: 'publication_conflict',
        });
        const disarm = () => adapter.preparedSchemaBarrier({arm: false});
        await withRestoration(async () => {
          await adapter.preparedSchemaBarrier({arm: true});
          record = await createAndOpen(admitted => {
            admission = admitted;
            pending = withRestoration(async () => {
              collision = await poll(() => race('inject'), value => value !== null,
                {timeout: 120000, interval: 100, code: 'prepared_publication_transaction_not_observed'});
            }, disarm, recordFailure).then(() => ({}), error => ({error}));
          });
          check(pending, 'prepared_publication_admission_unobserved');
          const outcome = await pending;
          if (outcome.error) throw outcome.error;
          check(collision?.agentId === record.agentId && collision?.sandboxId === record.sandboxId,
            'prepared_publication_collision_lineage_unbound');
        }, async () => {
          if (!admission) return disarm();
          await pending;
          const marker = await race('restore');
          check(marker.restored === 1 || (!collision && marker.restored === 0),
            'prepared_publication_collision_restore_unproven');
        }, recordFailure);
      } else {
        record = await createAndOpen();
      }
      check(Number.isSafeInteger(record.createToReadyMs) && record.createToReadyMs > 0
        && record.createToReadyMs < OLD_WORK_LEASE_MS, 'prepared_ready_work_lease_bound_unproven');
      const durable = await evidenceFor(record);
      check(ready.includes(record.sandboxId) && durable.consumed === true &&
        durable.installation_bound === true && durable.reservation_bound === true &&
        durable.agent_ready === true && !consumed.has(record.sandboxId),
      'prepared_admission_consumption_not_observed');
      consumed.add(record.sandboxId);
      const [sandbox, workspace, worktree] = durable.schemas ?? [];
      check(durable.schemas?.length === 3 && [1, 3, 4].includes(sandbox) &&
        [1, 3].includes(workspace) && worktree === 1, 'prepared_parent_schemas_unbound');
      check(durable.ready_fences?.length === 4 && durable.ready_fences.every(count => count === 1),
        'prepared_open_ready_atomic_publication_unproven');
      check(durable.startup_receipts?.length === 3 && durable.startup_receipts.every(count => count === 1),
        'prepared_startup_receipt_count_wrong');
      const nativeAfter = await adapter.preparedRuntimeEvidence({sandboxId: record.sandboxId, operationId: record.operationId});
      check(equal(nativeAfter, nativeBefore.get(record.sandboxId)) && ['indexed', 'legacy'].includes(nativeAfter?.format)
        && nativeAfter.pod_sha256 === hash(record.runtime.pod_uid) && nativeAfter.receipts?.length === 3
        && nativeAfter.receipts.every(isDigest) && new Set(nativeAfter.receipts).size === 3,
      'prepared_native_mutation_receipts_changed');
      const owner = assertOwner(await adapter.ownerOf({agentId: record.agentId}), record.agentId);
      const readyBeforeCollisionLeaseDeadline = collision && Number.isSafeInteger(record.readyObservedAt)
        && Number.isSafeInteger(collision.leaseDeadlineMs) && record.readyObservedAt < collision.leaseDeadlineMs;
      const browserOpen = await opening;
      if (browserOpen?.error) throw browserOpen.error;
      check(isDigest(browserOpen?.started), 'prepared_browser_starting_read_unproven');
      check(record.am === ingress && typeof record.admissionRequest === 'string'
        && record.admissionRequest.trim().length > 0, 'prepared_admission_ingress_unbound');
      await openBoth(record);
      const browserReply = await page.sendAndRead(record.agentId, adapter);
      check(browserReply?.effects === 1 && isDigest(browserReply.ready_request_sha256)
        && isDigest(browserReply.send_request_sha256) && isDigest(browserReply.answer_sha256)
        && ['agui', 'responses_v2'].includes(browserReply.transport),
        'prepared_browser_reply_unproven');
      const requests = {
        admission: hash(record.admissionRequest),
        browser_starting: browserOpen.started,
        browser_ready: browserReply.ready_request_sha256,
        browser_send: browserReply.send_request_sha256,
      };
      check(new Set(Object.values(requests)).size === Object.keys(requests).length, 'prepared_request_attribution_reused');
      const finalOwner = assertOwner(await adapter.ownerOf({agentId: record.agentId}), record.agentId);
      check(owner.instance_id && owner.instance_id === finalOwner.instance_id && owner.pod_uid === finalOwner.pod_uid
        && owner.epoch === finalOwner.epoch && owner.am === finalOwner.am, 'prepared_browser_owner_changed');
      const finalManagers = await servingManagers(adapter);
      check(['a', 'b'].every(am => finalManagers[am].instance_id === initialManagers[am].instance_id
        && finalManagers[am].pod_uid === initialManagers[am].pod_uid), 'prepared_browser_ingress_changed');
      check(finalManagers[owner.am].instance_id === owner.instance_id && finalManagers[owner.am].pod_uid === owner.pod_uid,
        'prepared_browser_owner_not_bound_to_manager');
      const publicationDiagnostics = await adapter.publicationEvidence({agentId: record.agentId});
      if (ingress === 'b') {
        const aborts = am => publicationDiagnostics[am]?.known_aborted_cas?.publish_connection ?? 0;
        const committedLeases = am => publicationDiagnostics[am]?.prepared_commit_work_lease_sha256 ?? [];
        const originalWorkLeaseCommitted = isDigest(collision.workLeaseSha256)
          && equal(committedLeases(owner.am), [collision.workLeaseSha256])
          && committedLeases(owner.am === 'a' ? 'b' : 'a').length === 0;
        recordAbortEvidence({ingress, owner: owner.am, owner_epoch: owner.epoch,
          owner_instance_sha256: hash(owner.instance_id), agent_sha256: hash(record.agentId),
          lifecycle_operation_sha256: hash(record.operationId), exact_reservation_conflict: true,
          collision_owner_epoch: collision.ownerEpoch,
          ready_before_collision_lease_deadline: readyBeforeCollisionLeaseDeadline,
          committed_with_collision_work_lease: originalWorkLeaseCommitted,
          publication_diagnostics: publicationDiagnostics});
        check(collision.ownerInstance === owner.instance_id && collision.ownerEpoch === owner.epoch,
          'prepared_publication_owner_changed_since_collision');
        check(readyBeforeCollisionLeaseDeadline, 'prepared_publication_original_work_lease_elapsed');
        check(aborts(owner.am) === 1 && aborts(owner.am === 'a' ? 'b' : 'a') === 0,
          'prepared_publication_abort_not_attributed');
        check(originalWorkLeaseCommitted, 'prepared_publication_work_holder_or_fence_changed');
      }
      await cleanup(record);
      return {ingress, owner: owner.am, owner_epoch: owner.epoch, parent_schema_versions: durable.schemas,
        create_to_ready_ms: record.createToReadyMs,
        ingress_instance_sha256: hash(ingressMember.instance_id), owner_instance_sha256: hash(owner.instance_id),
        request_sha256: requests, publication_diagnostics: publicationDiagnostics,
        observed_known_abort: ingress === 'b', ready_before_collision_lease_deadline: readyBeforeCollisionLeaseDeadline,
        atomic_publication_fence_count: durable.ready_fences.filter(count => count === 1).length,
        startup_receipt_count: durable.startup_receipts.length,
        runtime_receipt_format: nativeAfter.format, runtime_receipt_count: nativeAfter.receipts.length,
        browser_starting_read: true, browser_ready_read: true, browser_reply_sha256: browserReply.answer_sha256,
        browser_send_transport: browserReply.transport,
        prepared_reservation_sha256: hash(record.sandboxId), agent_sha256: hash(record.agentId),
        lifecycle_operation_sha256: hash(record.operationId)};
    }));
  }
}

export async function preparedParentEnvelopes({adapter, create, cleanup, step, recordFailure}) {
  for (const parentVersions of [[1, 1], [1, 3], [3, 1], [3, 3], [4, 1], [4, 3]]) {
    const ingress = parentVersions[1] === 1 ? 'a' : 'b';
    await step('prepared_parent_envelope_' + parentVersions.join('_'), async () => {
      const reservations = await poll(() => adapter.preparedReady(),
        rows => Array.isArray(rows) && rows.length > 0,
        {timeout: 180000, code: 'no_ready_prepared_reservation'});
      const originalRuntime = new Map(await Promise.all(reservations.map(async sandboxId =>
        [sandboxId, await adapter.preparedRuntimeEvidence({sandboxId})])));
      const disarm = () => adapter.preparedSchemaBarrier({arm: false});
      let record, installed, pending;
      await withRestoration(async () => {
        await adapter.preparedSchemaBarrier({arm: true, parentEnvelope: true});
        record = await create(ingress, false, {}, undefined, ({operationId}) => {
          pending = withRestoration(async () => {
            installed = await poll(() => adapter.preparedSchemaRace({
              action: 'inject', operationId, reservations, barrier: 'parent_envelope', parentVersions,
            }), value => value !== null, {timeout: 120000, interval: 100,
              code: 'prepared_parent_envelope_observation_missing'});
          }, disarm, recordFailure).then(() => ({}), error => ({error}));
        });
      }, () => pending || disarm(), recordFailure);
      check(pending, 'prepared_parent_envelope_admission_unobserved');
      const {error} = await pending;
      if (error) throw error;
      const {agentId, sandboxId, operationId} = record;
      check(installed?.agentId === agentId && installed.sandboxId === sandboxId
        && equal(installed.parentVersions, parentVersions) && reservations.includes(sandboxId)
        && installed.parentRevisions?.length === 2
        && installed.parentRevisions.every(revision => Number.isSafeInteger(revision) && revision > 1),
      'prepared_parent_envelope_lineage_unbound');
      const [durable, runtime] = await Promise.all([
        adapter.preparedEvidence({agentId, sandboxId, operationId}),
        adapter.preparedRuntimeEvidence({sandboxId, operationId}),
      ]);
      check(durable.consumed === true && durable.agent_ready === true && durable.reservation_bound === true
        && durable.installation_bound === true && equal(durable.schemas, [...parentVersions, 1])
        && equal(durable.ready_fences, [1, 1, 1, 1]) && equal(durable.startup_receipts, [1, 1, 1]),
      'prepared_parent_envelope_publication_unproven');
      check(equal(runtime, originalRuntime.get(sandboxId)) && runtime?.receipts?.length === 3
        && runtime.pod_sha256 === hash(record.runtime.pod_uid), 'prepared_parent_envelope_runtime_changed');
      const owner = assertOwner(await adapter.ownerOf({agentId}), agentId);
      const diagnostics = await adapter.publicationEvidence({agentId});
      const committed = diagnostics[owner.am];
      const peer = diagnostics[owner.am === 'a' ? 'b' : 'a'];
      const leases = committed?.prepared_commit_work_lease_sha256;
      check(leases?.length === 1 && isDigest(leases[0])
        && !peer?.prepared_commit_work_lease_sha256?.length
        && record.createToReadyMs > 0 && record.createToReadyMs < OLD_WORK_LEASE_MS,
      'prepared_parent_envelope_commit_unbound');
      check(equal(committed.prepared_commit_parent_revisions, [installed.parentRevisions]),
        'prepared_parent_envelope_committed_revisions_mismatch');
      await cleanup(record);
      return {ingress, owner: owner.am, owner_epoch: owner.epoch, parent_schema_versions: durable.schemas,
        prepared_commit_parent_revisions: installed.parentRevisions,
        create_to_ready_ms: record.createToReadyMs, atomic_publication_fence_count: durable.ready_fences.length,
        startup_receipt_count: durable.startup_receipts.length, runtime_receipt_count: runtime.receipts.length,
        lifecycle_operation_sha256: hash(operationId), prepared_reservation_sha256: hash(sandboxId),
        agent_sha256: hash(agentId)};
    });
  }
}

if (process.argv[1]?.endsWith('/prepared_suite.mjs')) {
  const config = await readConfig(process.stdin);
  const registry = JSON.parse(await readFile(new URL('./writer-regressions.json', import.meta.url)));
  let abortEvidence = null;
  const journey = await runJourney(config, {preparedHook: async context => {
    let browser;
    await withRestoration(async () => {
      await context.step('prepared_browser_available', async () => {
        browser = await openPreparedBrowser(config);
      });
      await preparedJourney({...context, browser,
        recordAbortEvidence: evidence => { abortEvidence = evidence; },
        recordFailure: (stage, error) => retainFailure(config.output_dir, stage, error,
          {case: 'prepared_publication_abort'})});
      await context.step('prepared_worktree_schema_refusal', () => preparedSchemaRefusal({...context,
        recordFailure: (stage, error) => retainFailure(config.output_dir, stage, error,
          {case: 'prepared_worktree_schema_refusal'})}));
      await context.step('prepared_worktree_cleanup_refusal', () => preparedSchemaRefusal({...context,
        barrier: 'worktree_cleanup', recordFailure: (stage, error) => retainFailure(config.output_dir, stage, error,
          {case: 'prepared_worktree_cleanup_refusal'})}));
      await context.step('prepared_work_lease_refusal', () => preparedSchemaRefusal({...context,
        barrier: 'work_lease', recordFailure: (stage, error) => retainFailure(config.output_dir, stage, error,
          {case: 'prepared_work_lease_refusal'})}));
      await preparedParentEnvelopes({...context,
        recordFailure: (stage, error) => retainFailure(config.output_dir, stage, error,
          {case: 'prepared_parent_envelope'})});
    }, () => browser?.close(), (stage, error) => retainFailure(config.output_dir, stage, error,
      {case: 'prepared_browser'}));
  }});
  const coverage = writerCoverage(registry, [], journey, 'prepared');
  const journeyVerdict = journey.verdict === 'LOCAL_PREPARED_TWO_AM_PASS' ? 'PASS' : 'FAIL';
  const verdict = journeyVerdict === 'PASS' && coverage.verdict === 'PASS' ? 'PASS' : 'FAIL';
  await writeFile(join(config.summary_dir, 'coverage.json'), JSON.stringify({
    schema: 'multi-am-prepared-coverage-v1', suite_verdict: verdict, source: config.expected_api_source,
    journey_verdict: journeyVerdict,
    ...journeyFailureSummary(journey),
    writer_coverage: coverage,
    prepared_publication_abort: abortEvidence,
    steps: journey.steps.map(({name, verdict, elapsed_ms, create_to_ready_ms, parent_schema_versions, owner_epoch, ingress, owner,
      ingress_instance_sha256, owner_instance_sha256, request_sha256, publication_diagnostics, observed_known_abort,
      ready_before_collision_lease_deadline, prepared_commit_parent_revisions,
      atomic_publication_fence_count, startup_receipt_count, runtime_receipt_format, runtime_receipt_count,
      browser_starting_read, browser_ready_read, browser_reply_sha256, browser_send_transport,
      prepared_reservation_sha256, agent_sha256, lifecycle_operation_sha256, prepared_schema_refused, prepared_cleanup_refused,
      prepared_work_lease_refused}) => ({
      name, verdict, elapsed_ms, create_to_ready_ms, parent_schema_versions, owner_epoch, ingress, owner,
      ingress_instance_sha256, owner_instance_sha256, request_sha256, publication_diagnostics, observed_known_abort,
      ready_before_collision_lease_deadline, prepared_commit_parent_revisions,
      atomic_publication_fence_count, startup_receipt_count, runtime_receipt_format, runtime_receipt_count,
      browser_starting_read, browser_ready_read, browser_reply_sha256, browser_send_transport,
      prepared_reservation_sha256, agent_sha256, lifecycle_operation_sha256, prepared_schema_refused, prepared_cleanup_refused,
      prepared_work_lease_refused,
    })),
  }, null, 2) + '\n', {flag: 'wx', mode: 0o600});
  process.exitCode = verdict === 'PASS' ? 0 : 1;
}
