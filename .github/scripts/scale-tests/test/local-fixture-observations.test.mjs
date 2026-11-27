import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdtemp, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import test from 'node:test';
import { hash } from '../lib/checks.mjs';
import { collectLocalFixtureObservation, qualifyProcessDeath, qualifyReceiverOutcomes } from '../lib/local-fixture-observations.mjs';
import { config as baseConfig } from './fixtures.mjs';

const podUid = 'test-pod';
const nodeId = 'b'.repeat(64);
const containerId = 'c'.repeat(64);
const runHash = hash('run');
const sourceHash = hash('receiver-source');
const target = { pod: 'receiver', container: 'receiver', podUidHash: hash(podUid) };
const config = { ...baseConfig, localObservation: {
  runHash, cluster: { kubeconfig: '/test/kubeconfig', namespaceHash: hash('isolated-namespace') },
  receiver: { ...target, sourceHash },
  process: { ...target, nodeContainerHash: hash(nodeId), engineContext: 'isolated-engine' },
} };

function receiverLine(number, outcome = 'flushed', correlationHash) {
  return `{"observed_ns":170000000000000000${number},"source_ip":"192.0.2.10","response_hash":"${hash('upstream')}",`
    + `"input_hash":"${hash('input')}","thread_hash":"${hash('thread')}","workload":true,"outcome":"${outcome}"`
    + (correlationHash ? `,"correlation_hash":"${correlationHash}"}\n` : '}\n');
}

function fixture() {
  const state = { logs: receiverLine(1), engine: 'unix:///var/run/test.sock', uid: podUid,
    runtimeId: containerId, runtimeState: 'CONTAINER_RUNNING', nodeCluster: 'cma-scale' };
  const calls = [];
  const command = async (file, args) => {
    calls.push({ file, args });
    const json = (value) => ({ stdout: JSON.stringify(value) });
    if (file === 'kubectl') {
      assert.ok(args.includes('kind-cma-scale') && args.includes('cma-scale'));
      if (args.includes('view')) return { stdout: 'https://127.0.0.1:8443' };
      if (args.includes('namespace')) return json({ metadata: { name: 'cma-scale', uid: 'isolated-namespace' } });
      if (args.includes('pods')) return json({ items: [{
        metadata: { uid: state.uid, name: 'receiver' }, spec: { nodeName: 'kind-worker' },
        status: { containerStatuses: [{ name: 'receiver', containerID: 'containerd://' + state.runtimeId, state: { running: {} } }] },
      }] });
      assert.ok(args.includes('logs') && args.includes('pod/receiver'));
      return { stdout: state.logs };
    }
    if (args[0] === 'context') return { stdout: state.engine };
    if (args[2] === 'inspect') return { stdout: [nodeId, '/kind-worker', state.nodeCluster].map(JSON.stringify).join('\n') };
    assert.deepEqual(args.slice(2, 8), ['exec', nodeId, 'crictl', 'inspect', '-o', 'go-template']);
    assert.equal(args.at(-1), containerId);
    return { stdout: [state.runtimeId, podUid, 'cma-scale', 'receiver', state.runtimeState,
      '2026-01-01T00:00:00.000000001Z', '2026-01-01T00:00:00.000000009Z'].map(JSON.stringify).join('\n') + '\n137\n111' };
  };
  return { state, calls, command };
}

test('receiver observes the exact append-only ledger and retains nanosecond precision', async () => {
  const { state, calls, command } = fixture();
  const before = await collectLocalFixtureObservation(config, 'receiver', 'start', { command });
  state.logs += receiverLine(2, 'disconnected');
  const after = await collectLocalFixtureObservation(config, 'receiver', 'finish', { before, command });
  assert.equal(before.count, 1);
  assert.equal(after.rows.length, 1);
  assert.equal(after.rows[0].ordinal, 2);
  assert.equal(after.rows[0].observedAtNs, '1700000000000000002');
  assert.equal(after.rows[0].outcome, 'disconnected');
  assert.equal(JSON.stringify(after).includes('192.0.2.10'), false);
  assert.ok(calls.every(({ args }) => !args.includes('exec')));
  state.logs = receiverLine(7) + receiverLine(2, 'disconnected');
  await assert.rejects(collectLocalFixtureObservation(config, 'receiver', 'finish', { before, command }),
    { code: 'local_observer_receiver_prefix_changed' });
  state.uid = 'replacement';
  await assert.rejects(collectLocalFixtureObservation(config, 'receiver', 'finish', { before, command }),
    { code: 'local_observer_pod_identity_changed' });
});

test('receiver qualification needs an independent command join for every observed workload row', async () => {
  const { state, command } = fixture();
  const before = await collectLocalFixtureObservation(config, 'receiver', 'start', { command });
  const marker = hash('exact-correlation-marker');
  state.logs += receiverLine(2, 'flushed', marker) + receiverLine(3, 'disconnected');
  const after = await collectLocalFixtureObservation(config, 'receiver', 'finish', { before, command });
  const records = after.rows.map(({ rowHash, upstreamResponseHash, correlationHash }, index) => ({
    rowHash, upstreamResponseHash, correlationHash, commandHash: hash('command' + index), ownerHash: hash('owner'),
    runtimeGeneration: 1, authorityEpoch: 1, observedSequence: 10 + index,
  }));
  const binding = { schema: 'cma-receiver-command-binding-v1', runHash, sourceHash: hash('binding'),
    observationHash: hash(JSON.stringify(after)), complete: true, records };
  const result = qualifyReceiverOutcomes(after, binding);
  assert.deepEqual(result.records.map(({ terminal }) => terminal), [true, false]);
  assert.throws(() => qualifyReceiverOutcomes(after, { ...binding, records: records.slice(1) }),
    { code: 'local_observer_receiver_command_binding_missing' });
  assert.equal(after.rows[0].correlationHash, marker);
  assert.throws(() => qualifyReceiverOutcomes(after, { ...binding, records: [{ ...records[0], correlationHash: hash('other-marker') }, records[1]] }),
    { code: 'local_observer_receiver_command_binding_missing' });
  assert.throws(() => qualifyReceiverOutcomes(after, { ...binding, sourceHash }),
    { code: 'local_observer_receiver_independent_source_required' });
  assert.throws(() => qualifyReceiverOutcomes(after, { ...binding, records: [{ ...records[0], upstreamResponseHash: hash('other') }, records[1]] }),
    { code: 'local_observer_receiver_command_binding_missing' });
});

test('process witness binds the original running sample, Kind node and exact CRI exit', async () => {
  const { state, command } = fixture();
  await assert.rejects(collectLocalFixtureObservation(config, 'process', 'exited', { command }),
    { code: 'local_observer_running_sample_required' });
  const before = await collectLocalFixtureObservation(config, 'process', 'running', { command });
  assert.equal(before.pid, 111);
  await assert.rejects(collectLocalFixtureObservation(config, 'process', 'exited', { before, command }),
    { code: 'local_observer_process_exit_unproven' });
  state.runtimeState = 'CONTAINER_EXITED';
  const exited = await collectLocalFixtureObservation(config, 'process', 'exited', { before, command });
  assert.equal(exited.processHash, before.processHash);
  assert.equal(BigInt(exited.exitedAtNs) - BigInt(before.startedAtNs), 8n);
  const binding = { schema: 'cma-process-death-ordering-v1', runHash, sourceHash: hash('ordering'),
    observationHash: hash(JSON.stringify(exited)), complete: true, processHash: exited.processHash,
    exitedAtNs: exited.exitedAtNs, commandHash: hash('command'), authorityOwnerHash: hash('owner'),
    runtimeGeneration: 1, authorityEpoch: 1, faultSequence: 7, deathObservedSequence: 9, abrupt: true };
  const proof = qualifyProcessDeath(exited, binding);
  assert.equal(proof.ownerHash, before.processHash);
  assert.equal(proof.processDeathObserved, true);
  assert.equal(proof.faultSequence, 7);
  assert.throws(() => qualifyProcessDeath(exited, { ...binding, exitedAtNs: before.startedAtNs }),
    { code: 'local_observer_process_death_order_invalid' });
  state.runtimeId = 'd'.repeat(64);
  await assert.rejects(collectLocalFixtureObservation(config, 'process', 'exited', { before, command }),
    { code: 'local_observer_runtime_identity_changed' });
});

test('process witness refuses a remote engine or an unowned Kind node before runtime inspection', async () => {
  const { state, calls, command } = fixture();
  state.engine = 'tcp://example.test:2376';
  await assert.rejects(collectLocalFixtureObservation(config, 'process', 'running', { command }),
    { code: 'local_observer_engine_not_local' });
  state.engine = 'unix:///var/run/test.sock';
  state.nodeCluster = 'unrelated-cluster';
  await assert.rejects(collectLocalFixtureObservation(config, 'process', 'running', { command }),
    { code: 'local_observer_kind_node_mismatch' });
  assert.ok(calls.every(({ args }) => !args.includes('exec')));
});


test('receiver capture refuses a container restart after an empty baseline', async () => {
  const { state, command } = fixture();
  state.logs = '';
  const before = await collectLocalFixtureObservation(config, 'receiver', 'start', { command });
  assert.equal(before.count, 0);
  state.logs = receiverLine(1);
  state.logs = receiverLine(2);
  state.runtimeId = 'd'.repeat(64);
  await assert.rejects(collectLocalFixtureObservation(config, 'receiver', 'finish', { before, command }),
    { code: 'local_observer_receiver_incarnation_changed' });
});

for (const kind of ['receiver', 'process']) {
  test(`qualification CLI rejects a different configured ${kind} target with valid evidence digests`, async (t) => {
    const directory = await realpath(await mkdtemp(join(tmpdir(), 'cma-collector-target-')));
    t.after(() => rm(directory, { recursive: true, force: true }));
    const { state, command } = fixture();
    const before = await collectLocalFixtureObservation(config, kind, kind === 'receiver' ? 'start' : 'running', { command });
    state.logs += receiverLine(2);
    state.runtimeState = 'CONTAINER_EXITED';
    const observation = await collectLocalFixtureObservation(config, kind, kind === 'receiver' ? 'finish' : 'exited', { before, command });
    const sourceHash = hash('independent-ordering');
    const binding = { runHash, sourceHash, observationHash: hash(JSON.stringify(observation)), complete: true };
    if (kind === 'receiver') {
      Object.assign(binding, { schema: 'cma-receiver-command-binding-v1', records: observation.rows.map((row) => ({
        rowHash: row.rowHash, upstreamResponseHash: row.upstreamResponseHash, commandHash: hash('command'), ownerHash: hash('owner'),
        runtimeGeneration: 1, authorityEpoch: 1, observedSequence: 5,
      })) });
    } else {
      Object.assign(binding, { schema: 'cma-process-death-ordering-v1', processHash: observation.processHash,
        exitedAtNs: observation.exitedAtNs, commandHash: hash('command'), authorityOwnerHash: hash('owner'),
        runtimeGeneration: 1, authorityEpoch: 1, faultSequence: 5, deathObservedSequence: 6, abrupt: true });
    }
    const input = async (name, value) => {
      const path = join(directory, name);
      const bytes = JSON.stringify(value) + '\n';
      await writeFile(path, bytes, { mode: 0o600 });
      return [path, hash(bytes)];
    };
    const [original] = await input('target-a.json', config);
    const [other] = await input('target-b.json', { ...config, localObservation: { ...config.localObservation,
      [kind]: { ...config.localObservation[kind], podUidHash: hash('other-pod'),
        ...(kind === 'receiver' ? { sourceHash: hash('other-receiver') } : { nodeContainerHash: hash('other-node') }) },
    } });
    const pinned = [...await input('observation.json', observation), ...await input('binding.json', binding)];
    const entrypoint = fileURLToPath(new URL('../local-fixture-observations.mjs', import.meta.url));
    const run = (selected, output) => promisify(execFile)(process.execPath,
      [entrypoint, selected, kind, 'qualify', join(directory, output), ...pinned]);
    assert.equal(JSON.parse((await run(original, 'accepted.json')).stdout).status, 'PASS');
    await assert.rejects(run(other, 'rejected.json'), (error) => {
      assert.equal(error.code, 2);
      assert.equal(JSON.parse(error.stderr).code, 'local_observer_target_identity_changed');
      return true;
    });
  });
}
