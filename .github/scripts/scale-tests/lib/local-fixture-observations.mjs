import { isIP } from 'node:net';
import { check, hash, isHash } from './checks.mjs';
import { executeWithInput, LocalCluster } from './kubernetes.mjs';
import { validateConfig } from './safety.mjs';

const namePattern = /^[a-z0-9][a-z0-9.-]{0,252}$/;
const enginePattern = /^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,127}$/;
const containerPattern = /^[a-f0-9]{64}$/;
const maximumBytes = 4 * 1024 * 1024;
const nodeProjection = String.raw`{{printf "%q\n%q\n%q" .Id .Name (index .Config.Labels "io.x-k8s.kind.cluster")}}`;
const runtimeProjection = String.raw`{{printf "%q\n%q\n%q\n%q\n%q\n%q\n%q\n%v\n%v" .status.id (index .status.labels "io.kubernetes.pod.uid") (index .status.labels "io.kubernetes.pod.namespace") .status.metadata.name .status.state .status.startedAt .status.finishedAt .status.exitCode .info.pid}}`;

function exactContainer(pod, target) {
  const status = pod.status?.containerStatuses?.find(({ name }) => name === target.container);
  check(status?.state?.running && /^containerd:\/\/[a-f0-9]{64}$/.test(status.containerID),
    'local_observer_container_not_running', true);
  return status.containerID.slice('containerd://'.length);
}

function selectPod(pods, target) {
  const pod = pods.items.find(({ metadata }) => metadata.name === target.pod);
  check(pod && !pod.metadata.deletionTimestamp && hash(pod.metadata.uid) === target.podUidHash,
    'local_observer_pod_identity_changed', true);
  return pod;
}

function exactNanoseconds(value) {
  if (typeof value === 'string' && /^\d{1,20}$/.test(value)) return BigInt(value);
  const rfc3339 = typeof value === 'string' && /^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.(\d{1,9}))?(Z|[+-]\d\d:\d\d)$/.exec(value);
  check(rfc3339, 'local_observer_timestamp_invalid', true);
  const millis = Date.parse(rfc3339[1] + rfc3339[3]);
  check(Number.isSafeInteger(millis), 'local_observer_timestamp_invalid', true);
  return BigInt(millis) * 1_000_000n + BigInt((rfc3339[2] ?? '').padEnd(9, '0'));
}

function parseExactTimestamps(bytes, keys) {
  const timestamp = new RegExp(`("(${keys.join('|')})"\\s*:\\s*)(\\d{1,20})(?=[,}\\s])`, 'g');
  return JSON.parse(bytes.replace(timestamp, '$1"$3"'));
}

async function dockerRead(command, args) {
  const { stdout } = await command('docker', args, {
    timeout: 15_000, maxBuffer: maximumBytes, encoding: 'utf8',
  });
  return stdout;
}

async function inspectRuntime(command, context, node, nodeContainerHash, runtimeId) {
  check(enginePattern.test(context) && namePattern.test(node) && containerPattern.test(runtimeId),
    'local_observer_runtime_scope_invalid', true);
  const endpoint = (await dockerRead(command, ['context', 'inspect', '--format', '{{.Endpoints.docker.Host}}', context])).trim();
  check(/^unix:\/\/\/[^\s]+$/.test(endpoint), 'local_observer_engine_not_local', true);
  const nodeOutput = await dockerRead(command, ['--context', context, 'inspect', '--type', 'container',
    '--format', nodeProjection, node]);
  const [nodeId, nodeName, kindCluster] = nodeOutput.trim().split('\n').map((line) => JSON.parse(line));
  check(nodeName === '/' + node && containerPattern.test(nodeId)
    && hash(nodeId) === nodeContainerHash && kindCluster === 'cma-scale', 'local_observer_kind_node_mismatch', true);
  const bytes = await dockerRead(command, ['--context', context, 'exec', nodeId, 'crictl', 'inspect',
    '-o', 'go-template', '--template', runtimeProjection, runtimeId]);
  const parts = bytes.trim().split('\n');
  check(parts.length === 9, 'local_observer_cri_projection_invalid', true);
  const [id, podUid, podNamespace, name, state, startedAt, finishedAt] = parts.slice(0, 7).map((line) => JSON.parse(line));
  return { id, podUid, podNamespace, name, state, startedAt, finishedAt, exitCode: Number(parts[7]), pid: Number(parts[8]) };
}

function checkRuntimeIdentity(status, binding, namespace) {
  check(status.id === binding.runtimeId && status.podUid === binding.podUid
    && status.podNamespace === namespace && status.name === binding.container, 'local_observer_runtime_identity_changed', true);
}

async function observeProcess(config, cluster, target, scope, phase, before, command) {
  let binding;
  if (phase === 'running') {
    const pod = selectPod(await cluster.read('pods'), target);
    binding = { podUid: pod.metadata.uid, node: pod.spec?.nodeName, runtimeId: exactContainer(pod, target), container: target.container };
  } else {
    await cluster.verify();
    binding = before?.binding;
    check(before?.schema === 'cma-cri-process-observation-v1' && before.phase === 'running'
      && Object.entries(scope).every(([key, value]) => before[key] === value)
      && binding?.container === target.container && typeof binding.podUid === 'string'
      && hash(binding.podUid) === target.podUidHash
      && Number.isSafeInteger(before.pid) && before.pid > 0,
    'local_observer_running_sample_required', true);
  }
  const status = await inspectRuntime(command, target.engineContext, binding.node, target.nodeContainerHash, binding.runtimeId);
  checkRuntimeIdentity(status, binding, config.namespace);
  const started = exactNanoseconds(status.startedAt);
  check(started > 0n, 'local_observer_process_start_missing', true);
  const identity = { schema: 'cma-cri-process-observation-v1', ...scope, phase, binding, startedAtNs: started.toString() };
  if (phase === 'running') {
    const { pid } = status;
    check(status.state === 'CONTAINER_RUNNING' && Number.isSafeInteger(pid) && pid > 0,
      'local_observer_process_not_running', true);
    const processHash = hash(JSON.stringify([scope.namespaceHash, scope.podUidHash, scope.nodeContainerHash,
      binding.runtimeId, identity.startedAtNs, pid]));
    return { ...identity, pid, processHash };
  }
  check(before.startedAtNs === identity.startedAtNs
    && before.processHash === hash(JSON.stringify([scope.namespaceHash, scope.podUidHash, scope.nodeContainerHash,
      binding.runtimeId, identity.startedAtNs, before.pid])), 'local_observer_process_incarnation_changed', true);
  check(status.state === 'CONTAINER_EXITED', 'local_observer_process_exit_unproven', true);
  const finished = exactNanoseconds(status.finishedAt);
  check(finished > started, 'local_observer_process_exit_time_invalid', true);
  return { ...identity, processHash: before.processHash, exitedAtNs: finished.toString(),
    exitCode: Number.isSafeInteger(status.exitCode) ? status.exitCode : null, runningEvidenceHash: hash(JSON.stringify(before)) };
}

function receiverRows(logs) {
  return logs.split('\n').filter(Boolean).map((line, index) => {
    const value = parseExactTimestamps(line, ['observed_ns']);
    check(isIP(value.source_ip) > 0 && isHash(value.response_hash) && isHash(value.input_hash) && isHash(value.thread_hash)
      && typeof value.workload === 'boolean' && ['flushed', 'disconnected'].includes(value.outcome)
      && (!Object.hasOwn(value, 'correlation_hash') || (value.workload && isHash(value.correlation_hash))),
    'local_observer_receiver_row_invalid', true);
    return {
      ordinal: index + 1, rowHash: hash(line), observedAtNs: exactNanoseconds(value.observed_ns).toString(),
      sourceIpHash: hash(value.source_ip), upstreamResponseHash: value.response_hash,
      inputHash: value.input_hash, threadHash: value.thread_hash, workload: value.workload, outcome: value.outcome,
      ...(value.correlation_hash && { correlationHash: value.correlation_hash }),
    };
  });
}

async function observeReceiver(cluster, target, scope, phase, before) {
  const pod = selectPod(await cluster.read('pods'), target);
  const runtimeId = exactContainer(pod, target);
  const incarnationHash = hash(runtimeId);
  if (phase === 'finish') {
    check(before?.incarnationHash === incarnationHash, 'local_observer_receiver_incarnation_changed', true);
  }
  const bytes = await cluster.invoke(['logs', 'pod/' + target.pod, '--container', target.container]);
  const after = selectPod(await cluster.read('pods'), target);
  check(exactContainer(after, target) === runtimeId, 'local_observer_receiver_restarted', true);
  const rows = receiverRows(bytes);
  const identity = { schema: 'cma-receiver-ledger-observation-v1', ...scope, incarnationHash };
  if (phase === 'start') return { ...identity, phase, count: rows.length, prefixHash: hash(JSON.stringify(rows)) };
  check(Object.entries(identity).every(([key, value]) => before?.[key] === value)
    && before.phase === 'start' && Number.isSafeInteger(before.count) && before.count >= 0
    && rows.length >= before.count && hash(JSON.stringify(rows.slice(0, before.count))) === before.prefixHash,
  'local_observer_receiver_prefix_changed', true);
  return { ...identity, phase, baselineHash: hash(JSON.stringify(before)), rows: rows.slice(before.count) };
}

function observationTarget(config, kind) {
  validateConfig(config);
  check(['receiver', 'process'].includes(kind), 'local_observer_mode_invalid', true);
  const observation = config.localObservation;
  check(observation?.cluster && isHash(observation.cluster.namespaceHash) && isHash(observation.runHash),
    'local_observer_scope_missing', true);
  const target = observation[kind];
  check(target && typeof target.pod === 'string' && typeof target.container === 'string'
    && namePattern.test(target.pod) && namePattern.test(target.container) && isHash(target.podUidHash),
  'local_observer_target_binding_missing', true);
  const common = { runHash: observation.runHash, namespaceHash: observation.cluster.namespaceHash,
    pod: target.pod, podUidHash: target.podUidHash, container: target.container };
  if (kind === 'receiver') {
    check(isHash(target.sourceHash), 'local_observer_receiver_source_missing', true);
    return { target, clusterBinding: observation.cluster, scope: { ...common, sourceHash: target.sourceHash } };
  }
  check(isHash(target.nodeContainerHash) && typeof target.engineContext === 'string' && enginePattern.test(target.engineContext),
    'local_observer_node_binding_missing', true);
  return { target, clusterBinding: observation.cluster,
    scope: { ...common, nodeContainerHash: target.nodeContainerHash, engineContext: target.engineContext } };
}

export function checkLocalFixtureObservationTarget(config, kind, observation) {
  const { scope } = observationTarget(config, kind);
  check(Object.entries(scope).every(([key, value]) => observation?.[key] === value),
    'local_observer_target_identity_changed', true);
}

export async function collectLocalFixtureObservation(config, kind, phase, { before, command = executeWithInput } = {}) {
  const { target, clusterBinding, scope } = observationTarget(config, kind);
  check((kind === 'receiver' && ['start', 'finish'].includes(phase)) || (kind === 'process' && ['running', 'exited'].includes(phase)),
    'local_observer_mode_invalid', true);
  const cluster = new LocalCluster({ ...clusterBinding, context: config.context, namespace: config.namespace }, command);
  return kind === 'receiver'
    ? observeReceiver(cluster, target, scope, phase, before)
    : observeProcess(config, cluster, target, scope, phase, before, command);
}

function orderedBinding(observation, binding, schema) {
  check(binding?.schema === schema && isHash(observation.runHash)
    && binding.runHash === observation.runHash && binding.observationHash === hash(JSON.stringify(observation))
    && isHash(binding.sourceHash) && binding.complete === true, 'local_observer_ordered_binding_missing', true);
}

export function qualifyReceiverOutcomes(observation, binding) {
  check(observation?.schema === 'cma-receiver-ledger-observation-v1' && observation.phase === 'finish'
    && isHash(observation.sourceHash) && isHash(observation.incarnationHash) && Array.isArray(observation.rows),
  'local_observer_receiver_delta_missing', true);
  orderedBinding(observation, binding, 'cma-receiver-command-binding-v1');
  check(binding.sourceHash !== observation.sourceHash && Array.isArray(binding.records),
    'local_observer_receiver_independent_source_required', true);
  const links = new Map();
  for (const row of binding.records) {
    check(isHash(row.rowHash) && !links.has(row.rowHash) && isHash(row.upstreamResponseHash) && isHash(row.commandHash)
      && isHash(row.ownerHash) && Number.isSafeInteger(row.runtimeGeneration) && row.runtimeGeneration > 0
      && Number.isSafeInteger(row.authorityEpoch) && row.authorityEpoch > 0
      && Number.isSafeInteger(row.observedSequence) && row.observedSequence >= 0,
    'local_observer_receiver_command_binding_invalid', true);
    links.set(row.rowHash, row);
  }
  const workloadRows = observation.rows.filter(({ workload }) => workload === true);
  check(workloadRows.length > 0, 'local_observer_receiver_workload_missing', true);
  const records = workloadRows.map((row) => {
    const link = links.get(row.rowHash);
    check(link && link.upstreamResponseHash === row.upstreamResponseHash
      && (!row.correlationHash || row.correlationHash === link.correlationHash),
    'local_observer_receiver_command_binding_missing', true);
    links.delete(row.rowHash);
    return { commandHash: link.commandHash, ownerHash: link.ownerHash, runtimeGeneration: link.runtimeGeneration,
      authorityEpoch: link.authorityEpoch, observedSequence: link.observedSequence, terminal: row.outcome === 'flushed' };
  });
  check(links.size === 0, 'local_observer_receiver_command_binding_extra', true);
  return { schema: 'cma-local-outcome-capture-v1', role: 'receiver', runHash: observation.runHash,
    sourceHash: hash(JSON.stringify([observation.sourceHash, binding.sourceHash])), complete: true, records };
}

export function qualifyProcessDeath(observation, binding) {
  check(observation?.schema === 'cma-cri-process-observation-v1' && observation.phase === 'exited'
    && isHash(observation.processHash) && isHash(observation.runningEvidenceHash)
    && exactNanoseconds(observation.exitedAtNs) > exactNanoseconds(observation.startedAtNs),
  'local_observer_process_exit_unproven', true);
  orderedBinding(observation, binding, 'cma-process-death-ordering-v1');
  check(binding.processHash === observation.processHash && binding.exitedAtNs === observation.exitedAtNs
    && isHash(binding.commandHash) && isHash(binding.authorityOwnerHash) && binding.abrupt === true
    && Number.isSafeInteger(binding.runtimeGeneration) && binding.runtimeGeneration > 0
    && Number.isSafeInteger(binding.authorityEpoch) && binding.authorityEpoch > 0
    && Number.isSafeInteger(binding.faultSequence) && binding.faultSequence >= 0
    && Number.isSafeInteger(binding.deathObservedSequence) && binding.deathObservedSequence >= binding.faultSequence,
  'local_observer_process_death_order_invalid', true);
  return { runHash: observation.runHash, commandHash: binding.commandHash, ownerHash: observation.processHash,
    authorityOwnerHash: binding.authorityOwnerHash, runtimeGeneration: binding.runtimeGeneration,
    authorityEpoch: binding.authorityEpoch, faultSequence: binding.faultSequence,
    deathObservedSequence: binding.deathObservedSequence, abrupt: true, processDeathObserved: true,
    evidenceHash: hash(JSON.stringify([binding.observationHash, binding])) };
}
