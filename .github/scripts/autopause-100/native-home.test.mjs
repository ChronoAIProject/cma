import test from 'node:test';
import assert from 'node:assert/strict';
import {createNativeReader} from './native-reader.mjs';
import {fixture, server} from './fixtures/native.mjs';
import {isNativeSuspended} from './lifecycle.mjs';
import {sameHome} from './engine.mjs';

test('retained volume users without matching ownership cannot prove suspension', async t => {
  for (const phase of ['Running', 'Succeeded', 'Failed']) {
    for (const owner of ['missing', 'foreign']) {
      const data = fixture();
      delete data.pod.metadata.labels;
      data.pod.metadata.ownerReferences = owner === 'missing' ? [] : [
        {...data.pod.metadata.ownerReferences[0], name: 'foreign', uid: 'foreign-uid'},
      ];
      data.pod.status.phase = phase;
      data.pod.metadata.deletionTimestamp = '2026-01-01T00:00:00Z';
      data.pods.items = [data.pod];
      const live = await server(t, data);
      await assert.rejects(createNativeReader({origin: live.origin, binding: data.binding}),
        /native_observation_pods_failed/);
    }
  }
});

test('a genuinely unrelated Pod cannot block a complete suspension observation', async t => {
  const data = fixture();
  delete data.pod.metadata.labels;
  delete data.pod.metadata.ownerReferences;
  data.pod.spec.volumes[0].persistentVolumeClaim.claimName = 'unrelated-home';
  data.pods.items = [data.pod];
  const live = await server(t, data);
  assert(isNativeSuspended(await createNativeReader({origin: live.origin, binding: data.binding})()));
});

test('Running observation binds the actual runtime mounts to the original PVC', async t => {
  const data = fixture('Running');
  const live = await server(t, data);
  const result = await createNativeReader({origin: live.origin, binding: data.binding})();
  sameHome({...data.binding, native_uid: data.binding.sandbox.uid}, result);
  assert.equal(result.pods.length, 1);
});

const changes = [
  spec => { spec.volumes[0].persistentVolumeClaim.claimName = 'foreign-home'; },
  spec => { delete spec.volumes; },
  spec => { spec.volumes = []; },
  spec => { spec.volumes[0].name = 'other'; },
  spec => { spec.volumes[0].emptyDir = {}; },
  spec => { spec.volumes[0].persistentVolumeClaim.readOnly = true; },
  spec => { spec.containers[0].volumeMounts.pop(); },
  spec => { spec.containers[0].volumeMounts[0].mountPath = '/wrong'; },
  spec => { spec.containers[0].volumeMounts[0].subPath = 'state'; },
  spec => { spec.containers[0].volumeMounts[0].subPathExpr = 'home'; },
  spec => { spec.containers[0].volumeMounts[0].readOnly = true; },
  spec => { spec.containers[0].volumeMounts[0].readOnly = null; },
  spec => { spec.containers[0].volumeMounts.push(null); },
  spec => { spec.containers[0].volumeDevices = [{name: 'home', devicePath: '/device'}]; },
  spec => { spec.containers.push({...structuredClone(spec.containers[0]), name: 'sidecar'}); },
  spec => { spec.ephemeralContainers = [{name: 'debug', volumeMounts: spec.containers[0].volumeMounts}]; },
  spec => { spec.containers[0].volumeMounts.push({name: 'other', mountPath: '/home/cma/shadow'}); },
  spec => { spec.volumes.push({name: 'other', emptyDir: {}}); },
];

test('template and actual Pod must each retain the volume and mount graph', async t => {
  for (const target of ['template', 'pod']) {
    for (const change of changes) {
      const data = fixture('Running');
      change(target === 'template' ? data.sandbox.spec.podTemplate.spec : data.pod.spec);
      const live = await server(t, data);
      await assert.rejects(createNativeReader({origin: live.origin, binding: data.binding}),
        new RegExp(`native_observation_${target === 'template' ? 'sandbox' : 'pods'}_failed`));
    }
  }
});

test('malformed unrelated volume evidence cannot silently establish zero home users', async t => {
  for (const volumes of [null, {}, [null], [{name: 'home', persistentVolumeClaim: {}}]]) {
    const data = fixture();
    data.pod.metadata.ownerReferences = [];
    delete data.pod.metadata.labels;
    data.pod.spec.volumes = volumes;
    data.pods.items = [data.pod];
    const live = await server(t, data);
    await assert.rejects(createNativeReader({origin: live.origin, binding: data.binding}),
      /native_observation_pods_failed/);
  }
});
