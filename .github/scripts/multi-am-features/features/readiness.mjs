import {randomUUID} from 'node:crypto';
import {assertOwner, assertRecovery, check, observeServingDuringRestart, poll} from '../cma/core.mjs';
import {retainFailure, withRestoration} from '../failure.mjs';

function readFromBoth(api, record) {
  const path = `/api/v1/agents/${record.agentId}`;
  return async () => {
    for (const am of ['a', 'b']) {
      const {body} = await api.rawRequest(am, 'GET', path);
      check(body.id === record.agentId && body.status === 'ready' && body.sandbox?.id === record.sandboxId,
        'baseline_manager_stopped_serving');
    }
  };
}

export async function neverReadyManagerCase({adapter, api, record}) {
  const observed = await observeServingDuringRestart(() => adapter.observeNeverReady({agentId: record.agentId}),
    readFromBoth(api, record), {interval: 2000});
  const witness = observed.restored;
  check(witness.never_ready_samples >= 5 && witness.third_member_registered === false &&
    witness.startup_witness_removed === true, 'never_ready_manager_evidence_incomplete');
  return {never_ready_samples: witness.never_ready_samples, useful_reads: observed.servingSamples};
}

export async function scaleManagerCase({adapter, api, record}) {
  const observed = await observeServingDuringRestart(() => adapter.observeScale(),
    readFromBoth(api, record), {interval: 2000});
  const scale = observed.restored;
  check(scale.controller === 'HorizontalPodAutoscaler' && scale.trigger === 'replica_bounds' &&
    scale.ready_managers.join(',') === '2,3,2', 'hpa_scale_trajectory_incomplete');
  const owner = assertOwner(await poll(() => adapter.ownerOf({agentId: record.agentId}), current => current !== null,
    {timeout: 23000, code: 'hpa_final_owner_unavailable'}), record.agentId);
  return {ready_managers: scale.ready_managers, controller: scale.controller,
    useful_reads_during_scale: observed.servingSamples, final_owner: owner.am};
}

export async function drainOwnerCase({adapter, api, record, config}) {
  const before = assertOwner(await adapter.ownerOf({agentId: record.agentId}), record.agentId);
  const peer = before.am === 'a' ? 'b' : 'a';
  const path = `/api/v1/agents/${record.agentId}/preferences`;
  const read = async () => (await api.rawRequest(peer, 'GET', path)).body;
  const saved = await read();
  check(Number.isSafeInteger(saved.revision), 'drain_baseline_revision_required');
  const started = performance.now();
  const drain = await adapter.drainOwner({agentId: record.agentId, owner: before});
  const title = 'After drain ' + randomUUID();
  let recovered, serving, takeoverMs, wrote = false;
  await withRestoration(async () => {
    check(drain.am === before.am && drain.readiness === 503 && drain.liveness === 200 && drain.drain_reason === true &&
      Number.isSafeInteger(drain.elapsed_ms) && drain.elapsed_ms <= 35000, 'manager_drain_bound');
    const written = (await api.rawRequest(peer, 'PATCH', path, {
      body: {expected_revision: saved.revision, title}, key: randomUUID(),
    })).body;
    check(written.title === title && written.revision === saved.revision + 1,
      'peer_write_after_drain_missing');
    wrote = true;
    recovered = await poll(() => adapter.ownerOf({agentId: record.agentId}),
      current => current?.am === peer && current.epoch > before.epoch,
      {timeout: 30000, code: 'peer_takeover_after_drain_timeout'});
    assertRecovery(before, recovered, record.agentId);
    takeoverMs = Math.ceil(performance.now() - started);
    check(takeoverMs <= 65000, 'drain_and_takeover_bound');
  }, async () => {
    serving = await observeServingDuringRestart(() => adapter.restartDrained({am: before.am}), async () => {
      const observed = await read();
      check(observed.title === (wrote ? title : saved.title), 'peer_read_changed_during_drain_restart');
    });
    check(serving.restored?.ready === true && typeof serving.restored.origin === 'string',
      'drained_manager_restart_failed');
    config.ams[before.am] = serving.restored.origin;
  }, (stage, error) => retainFailure(config.output_dir, stage, error, {case: 'owner_drain_and_readiness'}));
  const topology = await adapter.topology();
  check(topology.ams[before.am]?.ready === true && topology.ams[before.am].pod_uid !== before.pod_uid &&
    topology.ams[peer]?.ready === true && topology.ams[peer].pod_uid === recovered.pod_uid,
  'drained_manager_restart_topology_mismatch');
  const current = await read();
  check(current.title === title, 'drain_title_changed_before_restore');
  const restored = (await api.rawRequest(peer, 'PATCH', path, {
    body: {expected_revision: current.revision, title: saved.title ?? record.name}, key: randomUUID(),
  })).body;
  check(restored.title === (saved.title ?? record.name), 'drain_title_restore_refused');
  return {readiness_dropped: true, process_still_live: true, peer_advanced: true,
    drain_ms: drain.elapsed_ms, takeover_ms: takeoverMs, useful_reads_during_restart: serving.servingSamples};
}
