import test from 'node:test';
import assert from 'node:assert/strict';
import {nativeFixture} from './fixtures/native-vct.mjs';
import {fixture, server} from './fixtures/native.mjs';
import {createNativeReader} from './native-reader.mjs';
import {isNativeSuspended} from './lifecycle.mjs';
import {sameHome} from './engine.mjs';

test('both native VCT layouts preserve Claim, controller-owned PVC and runtime mounts', async t => {
  for (const layout of ['native-vct-empty-v1', 'native-vct-full-home-v2']) {
    for (const mode of ['Running', 'Suspended']) {
      const data = nativeFixture(mode, layout);
      const live = await server(t, data);
      const result = await createNativeReader({origin: live.origin, binding: data.binding})();
      sameHome({...data.binding, native_uid: data.binding.sandbox.uid}, result);
      assert.equal(isNativeSuspended(result), mode === 'Suspended');
      assert.equal(result.home_contract, layout);
      assert.equal(live.requests.length, 9);
      assert(live.requests.every(req => req.method === 'GET' && Object.hasOwn(data.routes, req.url)));
    }
  }
});

test('unknown, missing and mixed home contracts refuse before issuing a request', async t => {
  const live = await server(t);
  for (const change of [
    b => { delete b.home_contract; }, b => { b.home_contract.kind = 'guess-from-owner'; },
    b => { b.home_contract = {kind: 'native-vct-empty-v1'}; },
    b => { b.home_contract.claim.uid = ''; }, b => { b.home_contract.claim.name = '../foreign'; },
    b => { b.home_contract.claim.request_sha256 = 'unknown'; },
    b => { b.home_contract.claim.request_json_sha256 = 'unknown'; },
    b => { b.home_contract.pool_config_sha256 = 'unknown'; },
    b => { b.home_contract.storage_class = ''; }, b => { b.home_contract.capacity_gib = 0; },
    b => { b.pvcs[0].name = 'other-home'; },
  ]) {
    const data = nativeFixture(); change(data.binding);
    assert.throws(() => createNativeReader({origin: live.origin, binding: data.binding}));
  }
  const legacy = fixture(); legacy.binding.home_contract.capacity_gib = 4;
  assert.throws(() => createNativeReader({origin: live.origin, binding: legacy.binding}));
  assert.equal(live.requests.length, 0);
});

test('omitted Kubernetes false defaults retain the same writable home graph', async t => {
  for (const data of [fixture('Running'), nativeFixture('Running')]) {
    for (const spec of [data.sandbox.spec.podTemplate.spec, data.pod.spec]) {
      for (const volume of spec.volumes ?? []) delete volume.persistentVolumeClaim.readOnly;
      for (const container of [...spec.containers, ...(spec.initContainers ?? [])]) {
        for (const mount of container.volumeMounts) delete mount.readOnly;
      }
    }
    delete data.pvc.spec.volumeMode;
    const live = await server(t, data);
    const result = await createNativeReader({origin: live.origin, binding: data.binding})();
    assert.equal(result.pods.length, 1);
  }
});

const mutations = [
  ['claim', d => { d.claim.metadata.uid = 'replacement'; }],
  ['claim', d => { d.claim.metadata.labels['cma/control-plane'] = 'foreign'; }],
  ['claim', d => { d.claim.metadata.deletionTimestamp = '2026-01-01T00:00:00Z'; }],
  ['claim', d => { d.claim.status.sandbox.name = 'foreign'; }],
  ['claim', d => { d.claim.metadata.annotations['cma/native-home-origin'] = 'changed'; }],
  ['claim', d => { d.claim.metadata.annotations['cma/native-pool-config'] = 'changed'; }],
  ['claim', d => { d.claim.metadata.annotations['cma/pool-claim-request'] = 'changed'; }],
  ['claim', d => { d.claim.spec.env = [{name: 'UNADMITTED', value: 'fixture'}]; }],
  ['claim', d => { d.claim.spec.warmPoolRef.name = 'foreign'; }],
  ['sandbox', d => { d.sandbox.metadata.ownerReferences[0].uid = 'replacement'; }],
  ['sandbox', d => { d.sandbox.spec.volumeClaimTemplates = []; }],
  ['sandbox', d => { d.sandbox.spec.volumeClaimTemplates[0].spec.dataSource = {}; }],
  ['sandbox', d => { d.sandbox.spec.volumeClaimTemplates[0].metadata.annotations['cma/home-layout'] = 'other'; }],
  ['sandbox', d => { d.sandbox.spec.podTemplate.spec.volumes = d.pod.spec.volumes; }],
  ['sandbox', d => { d.sandbox.spec.podTemplate.spec.initContainers = []; }],
  ['pvcs', d => { d.pvc.metadata.ownerReferences = []; }],
  ['pvcs', d => { d.pvc.metadata.ownerReferences[0].uid = 'foreign'; }],
  ['pvcs', d => { d.pvc.metadata.labels['cma/pool-config'] = 'other'; }],
  ['pvcs', d => { d.pvc.metadata.annotations['cma/home-layout'] = 'other'; }],
  ['pvcs', d => { d.pvc.spec.storageClassName = 'other'; }],
  ['pvcs', d => { d.pvc.spec.resources.requests.storage = '8Gi'; }],
  ['pvcs', d => { d.pvc.spec.accessModes = ['ReadWriteMany']; }],
  ['pvcs', d => { d.pvc.spec.dataSourceRef = {kind: 'VolumeSnapshot', name: 'foreign'}; }],
  ['pods', d => { d.pod.spec.volumes[0].persistentVolumeClaim.claimName = 'foreign'; }],
  ['pods', d => { d.pod.spec.containers[0].volumeMounts[0].mountPath = '/home'; }],
  ['pods', d => { d.pod.spec.initContainers[0].volumeMounts[0].subPath = 'home'; }],
  ['pods', d => { d.pod.spec.initContainers[0].image = 'foreign'; }],
  ['pods', d => { d.pod.spec.initContainers[0].command[1] = '--initialize-home-tree'; }],
  ['pods', d => { d.pod.spec.initContainers[0].args = ['foreign']; }],
];

test('native VCT authority and storage mutations fail at their observation stage', async t => {
  for (const [stage, mutate] of mutations) {
    const data = nativeFixture('Running'); mutate(data);
    const live = await server(t, data);
    await assert.rejects(createNativeReader({origin: live.origin, binding: data.binding}),
      new RegExp(`native_observation_${stage}_failed`));
  }
});

test('native Claim is rechecked and a legacy PVC cannot impersonate native authority', async t => {
  const data = nativeFixture();
  const live = await server(t, data, (_req, _res, requests) => {
    if (requests.length === 6) data.claim.metadata.resourceVersion = 'changed';
    return false;
  });
  await assert.rejects(createNativeReader({origin: live.origin, binding: data.binding}),
    /native_observation_recheck_failed/);
  const mixed = nativeFixture();
  mixed.pvc.metadata = {...fixture().pvc.metadata, name: mixed.binding.pvcs[0].name};
  const other = await server(t, mixed);
  await assert.rejects(createNativeReader({origin: other.origin, binding: mixed.binding}),
    /native_observation_pvcs_failed/);
});
