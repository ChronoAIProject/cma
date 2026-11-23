import {isDeepStrictEqual} from 'node:util';
import {check, hash, isHash} from '../scale-tests/lib/checks.mjs';

const dns = value => typeof value === 'string' && value.length <= 63
  && /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(value);
const text = value => typeof value === 'string' && value.length > 0 && value.length <= 256
  && /^[A-Za-z0-9_.:-]+$/.test(value);
const nativeVersion = 'agents.x-k8s.io/v1beta1';

// Match the upstream tracking hint used by agent_sandbox/pods.rs. The hash
// discovers suspicious orphan Pods; only the full owner UID can validate them.
export function trackingHash(name) {
  let value = 2166136261;
  for (const byte of Buffer.from(name)) value = Math.imul(value ^ byte, 16777619) >>> 0;
  return value.toString(16).padStart(8, '0');
}

export function validateBinding(value) {
  check(value && dns(value.namespace?.name) && dns(value.namespace.uid)
    && dns(value.sandbox?.name) && dns(value.sandbox.uid), 'native_binding_required');
  check(text(value.sandbox_id) && text(value.home_identity), 'native_home_binding_required');
  check(dns(value.deployment) && isHash(value.home_request_sha256) && isHash(value.native_origin_sha256),
    'native_adoption_binding_required');
  check(typeof value.image === 'string' && value.image.length <= 512
    && /^[a-z0-9]+(?:[._:/-][a-z0-9]+)*@sha256:[a-f0-9]{64}$/.test(value.image), 'native_image_pin_required');
  check(Array.isArray(value.pvcs) && value.pvcs.length > 0 && value.pvcs.length <= 8
    && value.pvcs.every(pvc => pvc && dns(pvc.name) && dns(pvc.uid) && dns(pvc.volume))
    && new Set(value.pvcs.map(pvc => pvc.name)).size === value.pvcs.length
    && new Set(value.pvcs.map(pvc => pvc.uid)).size === value.pvcs.length,
  'native_pvc_bindings_required');
  return structuredClone(value);
}

export function resource(value, kind, expected, namespace) {
  const meta = value?.metadata;
  check(value?.kind === kind && value.apiVersion === (kind === 'Sandbox' ? nativeVersion : 'v1')
    && meta?.name === expected.name && meta.uid === expected.uid
    && (namespace === undefined ? meta.namespace === undefined : meta.namespace === namespace)
    && text(meta.resourceVersion) && !meta.deletionTimestamp, 'native_resource_identity_changed');
  return meta;
}

function ownerReferences(value) {
  const owners = value.metadata.ownerReferences ?? [];
  check(Array.isArray(owners) && owners.every(owner => owner && text(owner.uid)
    && text(owner.name) && text(owner.kind) && typeof owner.apiVersion === 'string'),
  'native_owner_evidence_invalid');
  return owners;
}

function owned(value, binding) {
  const owners = ownerReferences(value);
  check(owners.filter(owner => owner.controller === true).length === 1
    && owners.some(owner => owner.controller === true && owner.kind === 'Sandbox'
      && owner.apiVersion === nativeVersion && owner.name === binding.sandbox.name
      && owner.uid === binding.sandbox.uid), 'native_resource_owner_changed');
}

function runtimeImage(spec, binding) {
  const containers = spec?.containers;
  check(Array.isArray(containers) && containers.length > 0
    && containers.every(container => container && text(container.name))
    && new Set(containers.map(container => container.name)).size === containers.length,
  'native_containers_invalid');
  const runtime = containers.filter(container => container.name === 'runtime');
  check(runtime.length === 1 && runtime[0].image === binding.image, 'native_image_changed');
}

export function sandboxEvidence(value, binding) {
  const meta = resource(value, 'Sandbox', binding.sandbox, binding.namespace.name);
  check(meta.annotations?.['cma/sandbox'] === binding.sandbox_id
    && meta.annotations?.['cma/home-identity'] === binding.home_identity
    && typeof meta.annotations?.['cma/native-home-origin'] === 'string'
    && hash(meta.annotations['cma/native-home-origin']) === binding.native_origin_sha256,
  'native_home_identity_changed');
  check(Number.isSafeInteger(meta.generation) && meta.generation > 0, 'native_generation_invalid');
  check(['Running', 'Suspended'].includes(value.spec?.operatingMode), 'native_mode_invalid');
  runtimeImage(value.spec.podTemplate?.spec, binding);
  const volumes = value.spec.podTemplate.spec.volumes;
  check(Array.isArray(value.spec.volumeClaimTemplates) && value.spec.volumeClaimTemplates.length === 0
    && Array.isArray(volumes) && volumes.length === binding.pvcs.length
    && volumes.every(volume => volume?.persistentVolumeClaim?.readOnly === false
      && binding.pvcs.some(pvc => pvc.name === volume.persistentVolumeClaim.claimName))
    && new Set(volumes.map(volume => volume.persistentVolumeClaim.claimName)).size === binding.pvcs.length,
  'native_retained_volumes_changed');
  const conditions = value.status?.conditions;
  check(Array.isArray(conditions) && conditions.every(condition => condition
    && text(condition.type) && ['True', 'False', 'Unknown'].includes(condition.status)
    && (condition.observedGeneration === undefined || (Number.isSafeInteger(condition.observedGeneration)
      && condition.observedGeneration >= 0)))
    && new Set(conditions.map(condition => condition.type)).size === conditions.length,
  'native_conditions_invalid');
  if (value.spec.operatingMode === 'Suspended') {
    check(conditions.some(condition => condition.type === 'Suspended' && condition.status === 'True'
      && condition.observedGeneration === meta.generation), 'native_suspension_unproved');
  }
  // Never retain arbitrary annotations, environment variables or condition messages.
  return {resource_version: meta.resourceVersion, generation: meta.generation,
    operating_mode: value.spec.operatingMode,
    conditions: conditions.filter(condition => ['Ready', 'Suspended'].includes(condition.type))
      .map(({type, status, observedGeneration}) => ({type, status, observedGeneration}))};
}

export function pvcEvidence(value, expected, binding) {
  const meta = resource(value, 'PersistentVolumeClaim', expected, binding.namespace.name);
  // Adoption deliberately detaches the PVC from controller garbage collection.
  // The previously verified origin and request, not a live Pod, bind this home.
  check(ownerReferences(value).length === 0
    && meta.labels?.['cma/home-managed-by'] === binding.deployment
    && meta.annotations?.['cma/home-request-sha256'] === binding.home_request_sha256
    && typeof meta.annotations?.['cma/native-home-origin'] === 'string'
    && hash(meta.annotations['cma/native-home-origin']) === binding.native_origin_sha256,
  'native_pvc_adoption_changed');
  check(value.spec?.volumeName === expected.volume && value.status?.phase === 'Bound',
    'native_pvc_binding_changed');
  return {name: expected.name, uid: expected.uid, volume: expected.volume};
}

export function podEvidence(page, binding, limit) {
  check(page?.kind === 'PodList' && page.apiVersion === 'v1' && text(page.metadata?.resourceVersion)
    && (page.metadata.continue === undefined || page.metadata.continue === '')
    && (page.metadata.remainingItemCount === undefined || page.metadata.remainingItemCount === 0)
    && Array.isArray(page.items) && page.items.length <= limit, 'native_pod_page_incomplete');
  const names = new Set(); const ids = new Set(); const pods = [];
  for (const pod of page.items) {
    const meta = pod?.metadata;
    check(pod?.kind === 'Pod' && pod.apiVersion === 'v1' && dns(meta?.name) && dns(meta.uid)
      && meta.namespace === binding.namespace.name && text(meta.resourceVersion)
      && !names.has(meta.name) && !ids.has(meta.uid), 'native_pod_page_invalid');
    names.add(meta.name); ids.add(meta.uid);
    const owners = ownerReferences(pod);
    const related = owners.some(owner => owner.uid === binding.sandbox.uid
      || (owner.kind === 'Sandbox' && owner.name === binding.sandbox.name))
      || meta.labels?.['cma/runtime-name'] === binding.sandbox.name
      || meta.labels?.['agents.x-k8s.io/sandbox-name-hash'] === trackingHash(binding.sandbox.name);
    if (!related) continue;
    owned(pod, binding);
    runtimeImage(pod.spec, binding);
    check(['Pending', 'Running', 'Succeeded', 'Failed', 'Unknown'].includes(pod.status?.phase),
      'native_pod_status_missing');
    // Terminating and completed Pods still disprove release until they disappear.
    pods.push({name: meta.name, uid: meta.uid, phase: pod.status.phase,
      deleting: Boolean(meta.deletionTimestamp)});
  }
  return pods.sort((a, b) => a.name.localeCompare(b.name));
}

export function unchanged(before, after) {
  check(isDeepStrictEqual(before, after), 'native_observation_changed');
}
