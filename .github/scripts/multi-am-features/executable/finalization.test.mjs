import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp, writeFile, readFile, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {createHash} from 'node:crypto';
import {postDisposalFinalize, registryFingerprint} from './finalization.mjs';
import {coverageCounts} from './counts.mjs';
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
async function fixture(t) {
  const directory = await mkdtemp(join(tmpdir(), 'c275-disposal-')); t.after(() => rm(directory, {recursive: true, force: true}));
  const config = {run_id: 'exact-campaign', expected_api_source: 'a'.repeat(40), expected_api_image: 'local@sha256:' + 'b'.repeat(64),
    disposable_database_sha256: 'c'.repeat(64)};
  const row = {case_id: 'C275:disposal', family: 'user', required_steps: ['expected_result'], phase_minimum_runs: {steady: 3}};
  const registry = {catalog: {rows: [row]}, cases: new Map([[row.case_id, {id: row.case_id, fixtures: [], scope: 'global', steps: row.required_steps}]])};
  const binding = {source: config.expected_api_source, image: config.expected_api_image, campaign_id: config.run_id,
    disposable_database_sha256: config.disposable_database_sha256, registry_sha256: registryFingerprint(registry), fixtures: [],
    owner_restart: {verified: true, evidence_sha256: 'd'.repeat(64)}};
  const results = [1,2,3].map(iteration => {
    const am = iteration === 2 ? 'b' : 'a';
    const serving = {schema: 'c275-serving-v1', am, origin: 'http://127.0.0.1:' + (am === 'a' ? 1111 : 2222),
      pod_uid: am, instance_id: am, container_id: am, ready: true, observed_at: '2026-11-16T10:00:00.000Z'};
    return {id: row.case_id, family: 'user', source: binding.source, image: binding.image, campaign_id: config.run_id,
      iteration, phase: 'steady', status: 'PASS', requires_parent_cleanup: true, owner_proofs: [], cleanup: [],
      steps: [{id: 'expected_result', status: 'PASS', assertions: 1}], requests: [{am, request_id: 'actual-' + iteration,
        serving, serving_after: {...serving, observed_at: '2026-11-16T10:00:00.003Z'},
        started_at: '2026-11-16T10:00:00.001Z', ended_at: '2026-11-16T10:00:00.002Z',
        attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}}]};
  });
  const packet = {kind: 'executable_private_packet', binding, results, last_case_finished_at: Date.parse('2026-11-16T10:01:00Z')};
  const write = async (name, value) => {
    const path = join(directory, name + '.json'), bytes = JSON.stringify(value, null, 2) + '\n';
    await writeFile(path, bytes); return {path, sha256: hash(bytes)};
  };
  const packetArtifact = await write('original', packet);
  const disposal = {schema: 'multi-am-database-disposal-v1', campaign_id: config.run_id,
    source: binding.source, image: binding.image,
    database_sha256: config.disposable_database_sha256, database_disposed: true, owned_children: 0,
    status: 'PASS', completed_at: '2026-11-16T10:02:00Z'};
  const disposalArtifact = await write('disposal', disposal);
  const cleanup = {...disposal, schema: 'multi-am-parent-cleanup-v1', scope: 'LOCAL_DISPOSABLE_TWO_AM',
    source: binding.source, image: binding.image, disposal_evidence_sha256: disposalArtifact.sha256};
  const cleanupArtifact = await write('cleanup', cleanup);
  return {config, registry, packet, cleanup, disposal, write,
    options: {packetArtifact, cleanupArtifact, disposalArtifact, config, registry}};
}

test('post-disposal saved-packet finalization verifies artifacts and preserves original evidence', async t => {
  const f = await fixture(t), before = await readFile(f.options.packetArtifact.path);
  assert.equal(coverageCounts(f.registry, f.packet.results, f.packet.binding).roles.user.passing, 0);
  const result = await postDisposalFinalize(f.options);
  assert.equal(result.counts.roles.user.passing, 1); assert.equal(result.application_processes_started, 0);
  assert.deepEqual(result.results, f.packet.results); assert.deepEqual(await readFile(f.options.packetArtifact.path), before);
  assert.equal(f.packet.binding.final_cleanup, undefined);
  const packet = structuredClone(f.packet); packet.results.push({...packet.results[0], status: 'FAIL'});
  const failed = await postDisposalFinalize({...f.options, packetArtifact: await f.write('failed', packet)});
  assert.equal(failed.verdict, 'FAIL'); assert.equal(failed.counts.roles.user.passing, 0);
  assert.deepEqual(failed.results, packet.results);
});

test('post-disposal cannot use stale files, changed registry or foreign/future cleanup', async t => {
  const f = await fixture(t);
  await assert.rejects(postDisposalFinalize({...f.options,
    packetArtifact: {...f.options.packetArtifact, sha256: '0'.repeat(64)}}), /artifact_hash_mismatch/);
  for (const patch of [{campaign_id: 'other'}, {source: 'e'.repeat(40)}, {database_disposed: false},
    {database_sha256: 'f'.repeat(64)}, {owned_children: 1}, {completed_at: '2026-11-16T09:00:00Z'},
    {disposal_evidence_sha256: '0'.repeat(64)}]) {
    const cleanupArtifact = await f.write('wrong-cleanup', {...f.cleanup, ...patch});
    await assert.rejects(postDisposalFinalize({...f.options, cleanupArtifact}), /binding/);
  }
  const changed = structuredClone(f.registry); changed.catalog.rows[0].required_steps.push('extra');
  await assert.rejects(postDisposalFinalize({...f.options, registry: changed}), /saved_packet_binding/);
  const disposalArtifact = await f.write('wrong-disposal', {...f.disposal, owned_children: 1});
  const cleanupArtifact = await f.write('joined-wrong-disposal', {...f.cleanup, disposal_evidence_sha256: disposalArtifact.sha256});
  await assert.rejects(postDisposalFinalize({...f.options, cleanupArtifact, disposalArtifact}), /disposal_evidence_binding/);
});
