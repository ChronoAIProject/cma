import {fixture} from './native.mjs';
import {hash} from '../../scale-tests/lib/checks.mjs';

// Synthetic rendering of pool/model.rs + native_home.rs; no cluster identifiers.
export function nativeFixture(mode = 'Suspended', layout = 'native-vct-empty-v1') {
  const data = fixture(mode);
  const {binding, sandbox, pvc, pod, routes} = data;
  const poolHash = hash('synthetic-native-pool');
  const request = JSON.stringify({pool: {config_sha256: poolHash}, home: {volume_id: 'fixture-home'}, runtime_generation: 1});
  binding.pvcs[0].name = 'home-member';
  binding.home_contract = {kind: layout, storage_class: 'fixture-disk', capacity_gib: 4,
    pool_config_sha256: poolHash, claim: {name: 'fixture-claim', uid: 'claim-uid',
      request_sha256: hash('synthetic-claim-request-digest'), request_json_sha256: hash(request)}};
  const origin = JSON.stringify({claim: {name: 'fixture-claim', uid: 'claim-uid', sandbox_name: 'member'},
    sandbox_uid: 'member-uid', pvc_uid: 'home-uid'});
  binding.native_origin_sha256 = hash(origin);
  sandbox.metadata.annotations['cma/native-home-origin'] = origin;
  sandbox.metadata.ownerReferences = [{apiVersion: 'extensions.agents.x-k8s.io/v1beta1',
    kind: 'SandboxClaim', name: 'fixture-claim', uid: 'claim-uid', controller: true}];
  const storage = {accessModes: ['ReadWriteOncePod'], volumeMode: 'Filesystem',
    storageClassName: 'fixture-disk', resources: {requests: {storage: '4Gi'}}};
  sandbox.spec.volumeClaimTemplates = [{metadata: {name: 'home',
    labels: {'cma/pool-config': poolHash.slice(0, 52)}, annotations: {'cma/home-layout': layout}},
  spec: structuredClone(storage)}];
  delete sandbox.spec.podTemplate.spec.volumes;
  const fullHome = layout === 'native-vct-full-home-v2';
  for (const spec of [sandbox.spec.podTemplate.spec, pod.spec]) {
    spec.containers[0].volumeMounts[0].mountPath = fullHome ? '/home' : '/home/cma';
    spec.initContainers = [{name: 'initialize-home', image: binding.image,
      command: ['/usr/local/bin/layer4-runtime', fullHome ? '--initialize-home-tree' : '--initialize-home', '/mnt/cma-home'],
      volumeMounts: [{name: 'home', mountPath: '/mnt/cma-home', readOnly: false}]}];
  }
  pod.spec.volumes[0].persistentVolumeClaim.claimName = binding.pvcs[0].name;
  pvc.metadata.name = binding.pvcs[0].name;
  pvc.metadata.ownerReferences = structuredClone(pod.metadata.ownerReferences);
  pvc.metadata.labels = {'cma/pool-config': poolHash.slice(0, 52)};
  pvc.metadata.annotations = {'cma/home-layout': layout};
  pvc.spec = {...storage, volumeName: binding.pvcs[0].volume};
  const claim = {apiVersion: 'extensions.agents.x-k8s.io/v1beta1', kind: 'SandboxClaim',
    metadata: {name: 'fixture-claim', uid: 'claim-uid', namespace: 'fixture', resourceVersion: '12',
      labels: {'cma/control-plane': binding.deployment}, annotations: {
        'cma/native-home-origin': origin, 'cma/native-pool-config': binding.home_contract.claim.request_sha256,
        'cma/pool-claim-request': request}},
    spec: {warmPoolRef: {name: 'cma-pool-' + poolHash.slice(0, 48)}},
    status: {sandbox: {name: 'member'}}};
  delete routes['/api/v1/namespaces/fixture/persistentvolumeclaims/home'];
  routes['/api/v1/namespaces/fixture/persistentvolumeclaims/home-member'] = pvc;
  routes['/apis/extensions.agents.x-k8s.io/v1beta1/namespaces/fixture/sandboxclaims/fixture-claim'] = claim;
  return {...data, claim};
}
