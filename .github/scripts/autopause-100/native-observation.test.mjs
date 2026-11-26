import test from 'node:test';
import assert from 'node:assert/strict';
import {fixture} from './fixtures/native.mjs';
import {validateBinding, resource, sandboxEvidence, pvcEvidence, podEvidence, unchanged, trackingHash} from './native-observation.mjs';

test('retained native evidence matches current v1beta1 and detached PVC contracts', () => {
  const data = fixture();
  assert.deepEqual(validateBinding(data.binding), data.binding);
  assert.equal(resource(data.namespace, 'Namespace', data.binding.namespace).uid, 'namespace-uid');
  assert.equal(sandboxEvidence(data.sandbox, data.binding).operating_mode, 'Suspended');
  assert.deepEqual(pvcEvidence(data.pvc, data.binding.pvcs[0], data.binding), data.binding.pvcs[0]);
  assert.deepEqual(podEvidence(data.pods, data.binding, 128), []);
  assert.doesNotThrow(() => unchanged(data.namespace, structuredClone(data.namespace)));
});

test('invalid or incomplete fixture bindings fail before any observation', () => {
  const mutations = [
    b => { delete b.namespace; }, b => { b.namespace.name = '../other'; },
    b => { b.namespace.uid = ''; }, b => { b.sandbox.uid = null; },
    b => { b.sandbox_id = ''; }, b => { b.home_identity = 'bad\nvalue'; },
    b => { b.deployment = ''; }, b => { b.home_request_sha256 = 'x'; },
    b => { b.native_origin_sha256 = 'x'; }, b => { b.image = 'runtime:latest'; },
    b => { b.image = 'https://example.invalid/runtime@sha256:' + 'a'.repeat(64); },
    b => { b.pvcs = []; }, b => { b.pvcs = null; }, b => { b.pvcs = [null]; },
    b => { b.pvcs[0].volume = ''; }, b => { b.pvcs[0].uid = ''; },
    b => { b.pvcs.push({...b.pvcs[0]}); },
    b => { b.pvcs.push({...b.pvcs[0], name: 'other'}); },
  ];
  for (const change of mutations) {
    const data = fixture(); change(data.binding);
    assert.throws(() => validateBinding(data.binding));
  }
  assert.throws(() => validateBinding(null));
});

test('namespace or object replacement and unknown metadata never match', () => {
  for (const change of [
    v => { v.kind = 'Secret'; }, v => { v.apiVersion = 'v2'; },
    v => { v.metadata.uid = 'replacement'; }, v => { v.metadata.name = 'other'; },
    v => { v.metadata.namespace = 'other'; }, v => { delete v.metadata; },
    v => { v.metadata.resourceVersion = ''; },
    v => { v.metadata.deletionTimestamp = '2026-01-01T00:00:00Z'; },
  ]) {
    const data = fixture(); change(data.namespace);
    assert.throws(() => resource(data.namespace, 'Namespace', data.binding.namespace), /identity_changed/);
  }
});

test('stale conditions, changed home, image and volume mappings cannot prove suspension', () => {
  const changes = [
    v => { v.apiVersion = 'agents.x-k8s.io/v1alpha1'; },
    v => { v.metadata.annotations['cma/sandbox'] = 'foreign'; },
    v => { v.metadata.annotations['cma/home-identity'] = 'foreign'; },
    v => { delete v.metadata.annotations['cma/native-home-origin']; },
    v => { v.metadata.annotations['cma/native-home-origin'] = 'foreign'; },
    v => { v.metadata.generation = 0; }, v => { v.spec.operatingMode = 'Unknown'; },
    v => { delete v.spec.podTemplate; }, v => { v.spec.podTemplate.spec.containers = []; },
    v => { v.spec.podTemplate.spec.containers[0].name = 'other'; },
    v => { v.spec.podTemplate.spec.containers[0].image = 'runtime:latest'; },
    v => { v.spec.podTemplate.spec.containers.push(v.spec.podTemplate.spec.containers[0]); },
    v => { v.spec.volumeClaimTemplates = [{}]; },
    v => { delete v.spec.podTemplate.spec.volumes; },
    v => { v.spec.podTemplate.spec.volumes[0].persistentVolumeClaim.claimName = 'foreign'; },
    v => { v.spec.podTemplate.spec.volumes[0].persistentVolumeClaim.readOnly = true; },
    v => { v.status.conditions[0].observedGeneration = 1; },
    v => { v.status.conditions[0].observedGeneration = 'private-value'; },
    v => { v.status.conditions[0].status = 'False'; },
    v => { v.status.conditions[0].status = 'maybe'; },
    v => { v.status.conditions[0].type = ''; },
    v => { v.status.conditions.push(v.status.conditions[0]); },
    v => { v.status.conditions = []; }, v => { delete v.status; },
  ];
  for (const change of changes) {
    const data = fixture(); change(data.sandbox);
    assert.throws(() => sandboxEvidence(data.sandbox, data.binding));
  }
});

test('retained PVC requires the original adoption, bound volume and empty owner refs', () => {
  for (const change of [
    v => { v.metadata.ownerReferences = fixture().pod.metadata.ownerReferences; },
    v => { v.metadata.ownerReferences = [null]; },
    v => { v.metadata.ownerReferences = {}; },
    v => { v.metadata.labels['cma/home-managed-by'] = 'other'; },
    v => { v.metadata.annotations['cma/home-request-sha256'] = 'b'.repeat(64); },
    v => { v.metadata.annotations['cma/native-home-origin'] = 'changed'; },
    v => { delete v.metadata.annotations; },
    v => { v.spec.volumeName = 'replacement'; },
    v => { v.status.phase = 'Pending'; },
    v => { v.metadata.namespace = 'other'; },
  ]) {
    const data = fixture(); change(data.pvc);
    assert.throws(() => pvcEvidence(data.pvc, data.binding.pvcs[0], data.binding));
  }
  const data = fixture(); delete data.pvc.metadata.ownerReferences;
  assert.deepEqual(pvcEvidence(data.pvc, data.binding.pvcs[0], data.binding), data.binding.pvcs[0]);
});

test('a complete Pod page retains terminating and terminal Pods without requiring a label', () => {
  const data = fixture('Running');
  delete data.pod.metadata.labels;
  data.pod.metadata.deletionTimestamp = '2026-01-01T00:00:00Z';
  data.pod.status.phase = 'Succeeded';
  assert.deepEqual(podEvidence(data.pods, data.binding, 128), [
    {name: 'worker', uid: 'pod-uid', phase: 'Succeeded', deleting: true},
  ]);
  const unrelated = structuredClone(data.pod);
  unrelated.metadata = {...unrelated.metadata, name: 'unrelated', uid: 'other', ownerReferences: []};
  unrelated.spec.volumes[0].persistentVolumeClaim.claimName = 'unrelated-home';
  data.pods.items.push(unrelated);
  assert.equal(podEvidence(data.pods, data.binding, 128).length, 1);
});

test('truncated, ambiguous and malformed Pod pages fail closed', () => {
  for (const change of [
    v => { v.metadata.continue = 'next-page'; }, v => { v.metadata.remainingItemCount = 1; },
    v => { v.metadata.resourceVersion = ''; }, v => { v.kind = 'List'; },
    v => { v.apiVersion = 'v2'; }, v => { v.items = null; },
    v => { v.items = Array(129).fill(v.items[0]); },
    v => { v.items.push(structuredClone(v.items[0])); },
    v => { v.items[0].kind = 'Service'; },
    v => { v.items[0].apiVersion = 'v2'; },
    v => { v.items[0].kind = null; },
    v => { v.items[0].apiVersion = null; },
    v => { v.items[0].metadata.namespace = 'foreign'; },
    v => { v.items[0].metadata.ownerReferences[0].uid = 'replacement'; },
    v => { v.items[0].metadata.ownerReferences[0].name = 'foreign'; },
    v => { v.items[0].metadata.ownerReferences[0].apiVersion = 'other/v1'; },
    v => { v.items[0].metadata.ownerReferences[0].controller = false; },
    v => { v.items[0].metadata.ownerReferences.push(v.items[0].metadata.ownerReferences[0]); },
    v => { delete v.items[0].metadata.ownerReferences; },
    v => { delete v.items[0].status; }, v => { v.items[0].status.phase = 'private-value'; },
    v => { v.items[0].spec.containers[0].image = 'changed'; },
  ]) {
    const data = fixture('Running'); change(data.pods);
    assert.throws(() => podEvidence(data.pods, data.binding, 128));
  }
});

test('resource changes during observation are refused', () => {
  const data = fixture(); const after = structuredClone(data.sandbox);
  after.metadata.resourceVersion = '11';
  assert.throws(() => unchanged(data.sandbox, after), /native_observation_changed/);
});

test('upstream Pod tracking hashes detect orphans without granting ownership', () => {
  for (const [name, expected] of [['', '811c9dc5'], ['hello', '4f9f2cab'], ['a', 'e40c292c']]) {
    assert.equal(trackingHash(name), expected);
  }
  const data = fixture('Running');
  data.pod.metadata.ownerReferences = [];
  data.pod.metadata.labels = {'agents.x-k8s.io/sandbox-name-hash': trackingHash(data.binding.sandbox.name)};
  assert.throws(() => podEvidence(data.pods, data.binding, 128), /native_resource_owner_changed/);
});
