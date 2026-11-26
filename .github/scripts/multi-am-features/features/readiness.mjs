import {randomUUID} from 'node:crypto';
import {assertOwner, assertRecovery, check, observeServingDuringRestart, poll} from '../cma/core.mjs';
import {retainFailure, withRestoration} from '../failure.mjs';

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
  let recovered, serving, wrote = false;
  await withRestoration(async () => {
    check(drain.am === before.am && drain.readiness === 503 &&
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
    check(performance.now() - started <= 65000, 'drain_and_takeover_bound');
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
  return {readiness_dropped: true, peer_advanced: true, useful_reads_during_restart: serving.servingSamples};
}
