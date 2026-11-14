import { hash } from '../lib/checks.mjs';
import { userLimits } from '../lib/metrics.mjs';
import { catalogSha256, requiredRoutes } from '../lib/catalog.mjs';

export const config = {
  schema: 'cma-scale-v1', environment: 'local', context: 'kind-cma-scale', namespace: 'cma-scale',
  isolated: true, endpoints: { am: 'http://127.0.0.1:18080' }, recoveryMs: 30000, scaleTimeoutMs: 1000,
  requiredRoutes, catalogSha256, scalingPeak: 4,
  ...Object.fromEntries(['hardwareHash', 'fixtureHash', 'workloadHash', 'sourceHash', 'imagesHash']
    .map((key) => [key, hash(key)])),
};
export const fakeTime = () => ({ value: 0, now() { return this.value; }, async sleep(ms) { this.value += ms; } });
export const proof = (replicaHash = hash('replica-a')) => ({ ok: true, completed: true, evidenceHash: hash('proof'),
  servingReplicaHash: replicaHash, rendered: true, frameReceived: true, exclusive: true,
  liveEvent: true, durableContinuity: true });
export const oracle = () => ({ evidenceHash: hash('oracle'), independent: true, complete: true,
  acceptedHashes: [hash('command')], commands: [{ commandHash: hash('command'), accepted: true,
    executions: 1, effects: 1, staleWrites: 0, terminal: true, correctOwner: true }] });
export const snapshot = (ready = 2, users = 100) => ({ evidenceHash: hash('snapshot'), connectedUsers: users,
  openStreams: users, failedRequests: 0, droppedRequests: 0, streamGaps: 0, staleWrites: 0,
  am: { ready, desired: ready, hpa: true, metricsAvailable: true, min: 2, max: 5, pdb: true },
  eg: { ready, desired: ready, hpa: true, metricsAvailable: true, min: 2, max: 5, pdb: true },
  attemptedRequests: 100, hpaEventsObserved: true, actualReadsPerSecond: 10, actualTurnsPerMinute: 20,
  minimumConnectedUsers: users, minimumOpenStreams: users });
export const cohort = (phase, elapsedMs = 100) => ({ ...config, phase,
  startedAt: '2026-01-01T00:00:00Z', finishedAt: '2026-01-01T00:01:00Z',
  observedReplicas: phase === 'n1' ? 1 : phase === 'n2' ? 2 : '2-4-2',
  samples: Object.fromEntries([...Object.keys(userLimits), ...config.requiredRoutes].map((metric) =>
    [metric, Array.from({ length: 100 }, () => ({ ok: true, elapsedMs, rendered: true }))])) });

export const contractProof = () => ({ passed: true, productionPath: true, mongoReplicaSet: true,
  processHashes: [hash('process-a'), hash('process-b')], clientHashes: [hash('client-a'), hash('client-b')],
  evidenceHash: hash('contract'), acceptedLost: 0, duplicateEffects: 0, staleWrites: 0 });

export function fakeDriver(scalingPeak = 4) {
  let observations = 0;
  return {
    cleaned: 0, closed: false, stopped: 0,
    async preflight() { return { ...config, local: true, reviewed: true, syntheticUsers: true, mutationWindowGranted: true }; },
    async routes() { return [{ kind: 'replica', replicaHash: hash('replica-a') },
      { kind: 'replica', replicaHash: hash('replica-b') }, { kind: 'balancer' }]; },
    async create(route) { return { proof: proof(route.replicaHash) }; },
    async connect(agent, route) { return { proof: proof(route.replicaHash) }; },
    async action(path, agent, route) { return proof(route.replicaHash); },
    async confirmStream(stream) { return stream.proof ?? proof(); },
    async reconcile() { return oracle(); },
    async disconnect() {},
    async cleanup() { this.cleaned++; },
    async beginInFlight() { return { accepted: true, inFlight: true, leaseJustRenewed: true,
      commandHash: hash('command'), ownerHash: hash('owner'), evidenceHash: hash('boundary') }; },
    async killOwner(work) { return { ownerHash: work.ownerHash, abrupt: true,
      processDeathObserved: true, evidenceHash: hash('loss') }; },
    async observeRecovery() { return { clientRecovered: true, sustainedSuccess: true, accounted: true }; },
    async connectUsers() { observations = 0; return {}; },
    async snapshot() { return snapshot([2, scalingPeak, 2][observations++]); },
    async startStress() { return {}; },
    async stopStress() { this.stopped++; },
    async finishUsers() { return snapshot(); },
    async measureCohort(phase) { return { ...cohort(phase),
      ...(phase === 'scaling' ? { observedReplicas: `2-${scalingPeak}-2` } : {}) }; },
    async contractCase() { return contractProof(); },
    async lossTrial({ mode, trial }) { return { ...contractProof(), trialHash: hash(`${mode}:${trial}`),
      faultKind: mode, nodeLossObserved: mode === 'node', recoveryMs: 1000, sustainedSuccess: true, oracle: oracle() }; },
    async replicaStep({ am, eg }) { return { ...contractProof(), am, eg, minimumConnectedUsers: 100,
      minimumStreams: 100, failedRequests: 0, mongoHeadroom: 0.4, allClientsAccounted: true,
      terminatingProcessesWithinBound: true, oracle: oracle() }; },
    async partitionLoad() { return { ...contractProof(), partitions: 20, hotRate: 10, otherMinimumRate: 1 / 30,
      retainedEvents: 100000, activeEvents: 100, replyParts: 8, attachments: true, preStartExpiry: 0,
      unknown: 0, partial: 0, providerStartSamples: cohort('n1').samples.send, oracle: oracle() }; },
    async close() { this.closed = true; },
  };
}
