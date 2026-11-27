import { randomUUID } from 'node:crypto';
import { isAbsolute } from 'node:path';
import { pathToFileURL } from 'node:url';
import { check, clock, hash, isHash, reconcile, safeFailure, waitFor, withinDeadline } from './checks.mjs';
import { LocalCluster } from './kubernetes.mjs';
import { runLocalWorkload } from './local-workload.mjs';
import { connectedWorkloadSelection } from './report.mjs';
import { verifyLocalOutcomes } from './local-outcome-oracle.mjs';
import { guardedFetch, pinnedModule, validateConfig } from './safety.mjs';
import { suddenLoss } from '../scenarios/correctness.mjs';

export const localHpaSelection = 'local-hpa';
export const localLossSelection = 'local-replica-loss';
const roles = ['am', 'eg'];

function withHookDeadline(signal, timeoutMs, code, operation) {
  return withinDeadline((deadline) => {
    const bounded = signal ? AbortSignal.any([signal, deadline]) : deadline;
    bounded.throwIfAborted();
    return operation(bounded);
  }, timeoutMs, code);
}

function observedResource(list, uidHash, kind) {
  check(isHash(uidHash) && Array.isArray(list.items), 'local_hpa_resource_binding_missing', true);
  const item = list.items.find(({ metadata }) => metadata?.uid && hash(metadata.uid) === uidHash);
  check(item && !item.metadata.deletionTimestamp, `local_hpa_${kind}_missing`, true);
  return item;
}

async function hpaSnapshot(cluster, bindings) {
  const [deployments, hpas, budgets] = await Promise.all(
    ['deployments', 'horizontalpodautoscalers', 'poddisruptionbudgets'].map((resource) => cluster.read(resource)));
  return Object.fromEntries(roles.map((role) => {
    const binding = bindings[role];
    const deployment = observedResource(deployments, binding.deployment, 'deployment');
    const hpa = observedResource(hpas, binding.hpa, 'autoscaler');
    const pdb = observedResource(budgets, binding.pdb, 'pdb');
    const conditions = hpa.status?.conditions ?? [];
    check(hpa.spec.scaleTargetRef.kind === 'Deployment' && hpa.spec.scaleTargetRef.name === deployment.metadata.name
      && hpa.spec.minReplicas === 2 && hpa.spec.maxReplicas === 5
      && hpa.status.currentMetrics?.length > 0
      && conditions.some(({ type, status }) => type === 'ScalingActive' && status === 'True'),
    'local_hpa_metrics_unavailable', true);
    check(Number.isSafeInteger(deployment.metadata.generation) && Number.isSafeInteger(deployment.spec.replicas)
      && Number.isSafeInteger(hpa.status.desiredReplicas), 'local_hpa_replica_count_missing', true);
    const matchLabels = pdb.spec?.selector?.matchLabels;
    check(matchLabels && Object.keys(matchLabels).length > 0 && !pdb.spec.selector.matchExpressions?.length
      && Object.entries(matchLabels).every(([key, value]) => deployment.spec.template?.metadata?.labels?.[key] === value),
    'local_hpa_pdb_target_mismatch', true);
    return [role, {
      ready: deployment.status.readyReplicas ?? 0,
      desired: hpa.status.desiredReplicas,
      lastScaleTime: hpa.status.lastScaleTime ?? null,
      templateHash: hash(JSON.stringify(deployment.spec.template)),
      settled: deployment.status?.observedGeneration === deployment.metadata.generation
        && deployment.status?.replicas === deployment.spec.replicas
        && deployment.status?.updatedReplicas === deployment.spec.replicas
        && deployment.spec.replicas === hpa.status.desiredReplicas
        && pdb.status?.observedGeneration === pdb.metadata.generation
        && pdb.status?.expectedPods === deployment.spec.replicas && pdb.status?.desiredHealthy >= 1,
    }];
  }));
}

async function observeHpa(config, cluster, hooks, workload, time) {
  const { signal, done, status } = workload;
  const peak = config.scalingPeak ?? 4;
  const targets = config.localResilience.hpa;
  const observed = [];
  const finished = new AbortController();
  void done.then(() => finished.abort(), () => finished.abort());
  const read = async () => {
    signal.throwIfAborted();
    check(!finished.signal.aborted, 'local_hpa_workload_ended_early');
    const row = await hpaSnapshot(cluster, targets);
    signal.throwIfAborted();
    check(!finished.signal.aborted, 'local_hpa_workload_ended_early');
    const workloadStatus = status();
    check(workloadStatus.openStreams === 100 && workloadStatus.failedRequests === 0
      && workloadStatus.droppedRequests === 0, 'local_hpa_workload_degraded');
    check(roles.every((role) => row[role].ready <= peak && row[role].desired <= peak), 'local_hpa_peak_exceeded');
    const first = observed[0];
    check(!first || roles.every((role) => first[role].templateHash === row[role].templateHash),
      'local_hpa_rollout_observed');
    observed.push(row);
    return row;
  };
  const bothAt = (row, count) => roles.every((role) => row[role].settled && row[role].ready === count && row[role].desired === count);
  const wait = (count) => withinDeadline((deadline) =>
    waitFor(async () => {
      deadline.throwIfAborted();
      return read();
    }, (row) => bothAt(row, count), { timeoutMs: config.scaleTimeoutMs, intervalMs: 1000, time }),
  config.scaleTimeoutMs, 'local_hpa_transition_timeout');
  const initial = await read();
  check(bothAt(initial, 2), 'local_hpa_initial_replicas_not_two', true);
  let stress;
  try {
    stress = await withHookDeadline(signal, config.recoveryMs, 'local_hpa_stress_start_timeout',
      (signal) => hooks.startStress({ targetReplicas: peak, signal }));
    check(stress !== undefined && stress !== null, 'local_hpa_stress_receipt_missing', true);
    const up = await wait(peak);
    check(roles.every((role) => Number.isFinite(Date.parse(up[role].lastScaleTime))
      && Date.parse(up[role].lastScaleTime) > (Date.parse(initial[role].lastScaleTime) || 0)),
    'local_hpa_scale_up_not_observed');
    await withHookDeadline(undefined, config.recoveryMs, 'local_hpa_stress_stop_timeout',
      (signal) => hooks.stopStress(stress, { signal }));
    stress = undefined;
    const down = await wait(2);
    check(roles.every((role) => Date.parse(down[role].lastScaleTime) > Date.parse(up[role].lastScaleTime)),
      'local_hpa_scale_down_not_observed');
    return { initial, up, down, observations: observed.length, evidenceHash: hash(JSON.stringify(observed)) };
  } finally {
    if (stress !== undefined && stress !== null) {
      await withHookDeadline(undefined, config.recoveryMs, 'local_hpa_stress_stop_timeout',
        (signal) => hooks.stopStress(stress, { signal }));
    }
  }
}

async function captureOracle(config, clusterBinding, captures, acceptedHashes, runHash, dependencies) {
  const result = await verifyLocalOutcomes({
    ...config, outcome: { runHash, captures, origin: config.endpoints.am, cluster: clusterBinding },
  }, { ...dependencies, expectedWorkload: {
    receipts: new Set(acceptedHashes), sha256: hash(JSON.stringify([...acceptedHashes].sort())),
  } });
  return result;
}

function captureWorkload(config, clusterBinding, hooks, dependencies) {
  return {
    begin: (identity, { signal }) => withHookDeadline(signal, config.recoveryMs, 'local_capture_start_timeout',
      (signal) => hooks.beginCapture(identity, { signal })),
    async finish(identity, acceptedHashes, options) {
      const { signal, readCommand } = options;
      const captures = await withHookDeadline(signal, config.scaleTimeoutMs, 'local_capture_finish_timeout',
        (signal) => hooks.finishCapture(identity, { signal, acceptedHashes }));
      signal.throwIfAborted();
      return reconcile(await captureOracle(config, clusterBinding, captures, acceptedHashes, identity.runHash,
        { ...dependencies, readCommand }));
    },
  };
}

async function runLoss(config, clusterBinding, hooks, mode, dependencies) {
  const { time = clock, signal } = dependencies;
  const runHash = hash(randomUUID());
  const identity = { runHash, fixtureHash: config.fixtureHash, sourceHash: config.sourceHash };
  try {
    await withHookDeadline(signal, config.recoveryMs, 'local_capture_start_timeout',
      (signal) => hooks.beginCapture(identity, { signal }));
    return await suddenLoss({
      beginInFlight: () => withHookDeadline(signal, config.scaleTimeoutMs, 'local_loss_admission_timeout',
        (signal) => hooks.beginInFlight(mode, { signal, runHash })),
      async killOwner(work, options) {
        const faultSignal = signal ? AbortSignal.any([signal, options.signal]) : options.signal;
        faultSignal.throwIfAborted();
        const receipt = await hooks.injectLoss(work, { ...options, signal: faultSignal });
        check(receipt?.ownerHash === work.ownerHash && isHash(receipt.evidenceHash),
          'local_loss_injection_receipt_mismatch');
        faultSignal.throwIfAborted();
        const death = await hooks.observeDeath(work, { ...options, signal: faultSignal });
        check(death?.evidenceHash !== receipt.evidenceHash, 'local_loss_observation_reuses_injection');
        return death;
      },
      observeRecovery: (work, options) => hooks.observeRecovery(work, {
        ...options, signal: signal ? AbortSignal.any([signal, options.signal]) : options.signal,
      }),
      confirmStream: (work) => withHookDeadline(signal, config.recoveryMs, 'local_loss_stream_timeout',
        (signal) => hooks.confirmStream(work, { signal })),
      cleanup: (work) => withHookDeadline(undefined, config.recoveryMs, 'local_loss_cleanup_timeout',
        (signal) => hooks.cleanupFault(work, { signal })),
      async reconcile(work) {
        const acceptedHashes = [work.commandHash];
        const captures = await withHookDeadline(signal, config.scaleTimeoutMs, 'local_capture_finish_timeout',
          (signal) => hooks.finishCapture(identity, { acceptedHashes, signal }));
        signal?.throwIfAborted();
        return captureOracle(config, clusterBinding, captures, acceptedHashes, runHash, dependencies);
      },
    }, mode, config, time);
  } finally {
    await withHookDeadline(undefined, config.recoveryMs, 'local_capture_cleanup_timeout',
      (signal) => hooks.closeCapture(identity, { signal }));
  }
}

export async function runLocalResilience(config, selection, dependencies = {}) {
  let hooks;
  try {
    validateConfig(config);
    check([localHpaSelection, localLossSelection, connectedWorkloadSelection].includes(selection), 'local_resilience_selection_invalid', true);
    const resilience = config.localResilience;
    const clusterBinding = config.localCohort?.cluster;
    check(clusterBinding && resilience && isAbsolute(resilience.hookModule ?? ''),
      'local_resilience_fixture_missing', true);
    const cluster = new LocalCluster({ ...clusterBinding, context: config.context, namespace: config.namespace },
      dependencies.command);
    await cluster.verify();
    dependencies.signal?.throwIfAborted();
    const module = await pinnedModule(pathToFileURL(resilience.hookModule), resilience.hookSha256, dependencies.importer);
    check(typeof module.createLocalHooks === 'function', 'local_resilience_hooks_missing', true);
    hooks = await module.createLocalHooks(config, { fetch: guardedFetch(dependencies.fetchImpl) });
    const common = ['preflight', 'beginCapture', 'finishCapture', 'closeCapture', 'close'];
    const hpa = selection === localHpaSelection;
    const loss = selection === localLossSelection;
    const required = hpa ? ['startStress', 'stopStress'] : loss ? [
      'beginInFlight', 'injectLoss', 'observeDeath', 'observeRecovery', 'confirmStream', 'cleanupFault',
    ] : [];
    check([...common, ...required].every((name) => typeof hooks?.[name] === 'function'),
      'local_resilience_hooks_incomplete', true);

    const preflight = await withHookDeadline(dependencies.signal, config.recoveryMs, 'local_resilience_preflight_timeout',
      (signal) => hooks.preflight({ selection, namespaceHash: clusterBinding.namespaceHash, signal }));
    check(preflight?.ready === true && preflight.namespaceHash === clusterBinding.namespaceHash
      && preflight.sourceHash === config.sourceHash && isHash(preflight.evidenceHash), 'local_resilience_hook_preflight_failed', true);
    if (!loss) {
      if (hpa) check(roles.every((role) => ['deployment', 'hpa', 'pdb'].every((key) => isHash(resilience.hpa?.[role]?.[key]))),
        'local_hpa_resource_binding_missing', true);
      let scale;
      let captureIdentity;
      const capture = captureWorkload(config, clusterBinding, hooks, dependencies);
      try {
        const workload = await runLocalWorkload(config, {
          ...dependencies,
          capture: { ...capture, begin: (identity, options) => {
            captureIdentity = identity;
            return capture.begin(identity, options);
          } },
          onRunning: hpa ? async (state) => { scale = await observeHpa(config, cluster, hooks, state, dependencies.time ?? clock); }
            : undefined,
        });
        return hpa ? { status: workload.status, code: workload.code, workload, scale } : workload;
      } finally {
        if (captureIdentity) await withHookDeadline(undefined, config.recoveryMs, 'local_capture_cleanup_timeout',
          (signal) => hooks.closeCapture(captureIdentity, { signal }));
      }
    }
    const mode = resilience.lossMode;
    check(['am_stream', 'am_command', 'eg_inbound'].includes(mode), 'local_loss_mode_missing', true);
    return { status: 'PASS', fault: await runLoss(config, clusterBinding, hooks, mode, dependencies) };
  } catch (error) {
    return safeFailure(error);
  } finally {
    if (hooks?.close) await withHookDeadline(undefined, config.recoveryMs, 'local_resilience_cleanup_timeout',
      (signal) => hooks.close({ signal }));
  }
}
