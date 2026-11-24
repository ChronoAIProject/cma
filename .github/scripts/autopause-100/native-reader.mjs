import {LocalHttp} from '../scale-tests/lib/http.mjs';
import {check, hash, safeFailure} from '../scale-tests/lib/checks.mjs';
import {validateBinding, resource, claimEvidence, sandboxEvidence, pvcEvidence, podEvidence, unchanged} from './native-observation.mjs';
import {homeContract} from './native-contract.mjs';

const MAX_PODS = 128;

// Observation only: callers must obtain this binding from independently verified
// fixture custody. A matching snapshot never authorizes pause, delete or adoption.
export function createNativeReader({origin, binding, timeoutMs = 10000, bearer, fetchImpl} = {}) {
  const pinned = validateBinding(binding);
  check(Number.isSafeInteger(timeoutMs) && timeoutMs > 0 && timeoutMs <= 30000,
    'native_observation_budget_invalid');
  const http = new LocalHttp(origin, {bearer, fetchImpl, timeoutMs});
  check(['http:', 'https:'].includes(new URL(http.origin).protocol), 'native_http_origin_required');
  const namespacePath = `/api/v1/namespaces/${pinned.namespace.name}`;
  const sandboxPath = `/apis/agents.x-k8s.io/v1beta1/namespaces/${pinned.namespace.name}/sandboxes/${pinned.sandbox.name}`;
  const pvcPath = name => `${namespacePath}/persistentvolumeclaims/${name}`;
  const claimPath = homeContract(pinned).native
    ? `/apis/extensions.agents.x-k8s.io/v1beta1/namespaces/${pinned.namespace.name}/sandboxclaims/${pinned.home_contract.claim.name}` : null;
  const claimRoute = '/apis/extensions.agents.x-k8s.io/v1beta1/namespaces/:namespace/sandboxclaims/:claim';

  return async function readNativeSnapshot({signal} = {}) {
    const started = performance.now();
    const budget = AbortSignal.timeout(timeoutMs);
    const combined = signal ? AbortSignal.any([signal, budget]) : budget;
    let stage = 'namespace';
    const samples = [];
    const read = async (path, route) => {
      combined.throwIfAborted();
      const response = await http.request(path, {route, signal: combined});
      combined.throwIfAborted();
      samples.push({route, sha256: response.evidenceHash});
      // LocalHttp keeps timing samples for load runs; this reader has a bounded
      // lifetime per snapshot and must not grow a process-wide sample backlog.
      http.samples.length = 0;
      return response.value;
    };
    try {
      const namespace = await read(namespacePath, '/api/v1/namespaces/:namespace');
      resource(namespace, 'Namespace', pinned.namespace);
      let claim;
      if (claimPath) {
        stage = 'claim';
        claim = await read(claimPath, claimRoute);
        claimEvidence(claim, pinned);
      }
      stage = 'sandbox';
      const sandbox = await read(sandboxPath, '/apis/agents.x-k8s.io/v1beta1/namespaces/:namespace/sandboxes/:sandbox');
      const observed = sandboxEvidence(sandbox, pinned);
      stage = 'pvcs';
      const beforePvcs = [];
      const pvcs = [];
      for (const expected of pinned.pvcs) {
        const value = await read(pvcPath(expected.name), '/api/v1/namespaces/:namespace/persistentvolumeclaims/:pvc');
        beforePvcs.push(value);
        pvcs.push(pvcEvidence(value, expected, pinned));
      }
      stage = 'pods';
      // A complete namespace page also catches Pods whose runtime label is lost.
      // Exceeding the bound refuses; filtering a truncated list could invent zero.
      const page = await read(`${namespacePath}/pods?limit=${MAX_PODS}`, '/api/v1/namespaces/:namespace/pods');
      const pods = podEvidence(page, pinned, MAX_PODS);
      check(observed.operating_mode !== 'Suspended' || pods.length === 0, 'native_pods_not_released');
      stage = 'recheck';
      for (let index = 0; index < pinned.pvcs.length; index += 1) {
        unchanged(beforePvcs[index], await read(pvcPath(pinned.pvcs[index].name),
          '/api/v1/namespaces/:namespace/persistentvolumeclaims/:pvc'));
      }
      unchanged(sandbox, await read(sandboxPath,
        '/apis/agents.x-k8s.io/v1beta1/namespaces/:namespace/sandboxes/:sandbox'));
      if (claimPath) unchanged(claim, await read(claimPath, claimRoute));
      unchanged(namespace, await read(namespacePath, '/api/v1/namespaces/:namespace'));
      combined.throwIfAborted();
      return {sandbox_id: pinned.sandbox_id, native_uid: pinned.sandbox.uid, home_contract: pinned.home_contract.kind,
        home_identity: pinned.home_identity, pvcs: pvcs.sort((a, b) => a.name.localeCompare(b.name)),
        pods, ...observed, evidence: {scope: 'READ_ONLY_OBSERVATION', qualification: 'NOT_ASSESSED',
          binding_sha256: hash(JSON.stringify(pinned)), observed_at: new Date().toISOString(),
          elapsed_ms: performance.now() - started, reads: samples}};
    } catch (error) {
      // Native API bodies and network errors may contain private configuration.
      const failure = new Error(`native_observation_${stage}_failed`);
      failure.code = failure.message;
      failure.reason = safeFailure(error).code;
      throw failure;
    } finally {
      http.samples.length = 0;
    }
  };
}
