import {createServer} from 'node:http';
import {once} from 'node:events';
import {hash} from '../../scale-tests/lib/checks.mjs';

export function fixture(mode = 'Suspended') {
  const origin = JSON.stringify({fixture: 'synthetic-adopted-home'});
  const binding = {
    namespace: {name: 'fixture', uid: 'namespace-uid'},
    sandbox: {name: 'member', uid: 'member-uid'},
    sandbox_id: 'sbx_fixture', home_identity: 'fixture-home', deployment: 'fixture-manager',
    home_contract: {kind: 'legacy-detached-v1'},
    home_request_sha256: hash('synthetic-home-request'), native_origin_sha256: hash(origin),
    image: 'example.invalid/runtime@sha256:' + 'a'.repeat(64),
    pvcs: [{name: 'home', uid: 'home-uid', volume: 'volume'}],
  };
  const metadata = (name, uid) => ({name, uid, namespace: 'fixture', resourceVersion: '10'});
  // Match pool/model.rs: the retained home holds the runtime's three subtrees.
  const volumeMounts = [
    {name: 'home', mountPath: '/home/cma', subPath: 'home', readOnly: false},
    {name: 'home', mountPath: '/var/lib/cma-runtime', subPath: 'state', readOnly: false},
    {name: 'home', mountPath: '/var/lib/cma-transfer', subPath: 'transfer', readOnly: false},
  ];
  const containers = [{name: 'runtime', image: binding.image, volumeMounts,
    env: [{name: 'PRIVATE_TEST_VALUE', value: 'omit-this-field'}]}];
  const namespace = {apiVersion: 'v1', kind: 'Namespace',
    metadata: {name: 'fixture', uid: 'namespace-uid', resourceVersion: '1'}};
  const sandbox = {apiVersion: 'agents.x-k8s.io/v1beta1', kind: 'Sandbox',
    metadata: {...metadata('member', 'member-uid'), generation: 2, annotations: {
      'cma/sandbox': binding.sandbox_id, 'cma/home-identity': binding.home_identity,
      'cma/native-home-origin': origin, 'private-fixture-field': 'omit-this-field',
    }},
    spec: {operatingMode: mode, volumeClaimTemplates: [], podTemplate: {spec: {containers,
      volumes: [{name: 'home', persistentVolumeClaim: {claimName: 'home', readOnly: false}}]}}},
    status: {conditions: [{type: mode === 'Suspended' ? 'Suspended' : 'Ready', status: 'True',
      observedGeneration: 2, message: 'omit-this-field'}]},
  };
  const pvc = {apiVersion: 'v1', kind: 'PersistentVolumeClaim',
    metadata: {...metadata('home', 'home-uid'), ownerReferences: [],
      labels: {'cma/home-managed-by': binding.deployment}, annotations: {
        'cma/native-home-origin': origin, 'cma/home-request-sha256': binding.home_request_sha256,
      }}, spec: {volumeName: 'volume'}, status: {phase: 'Bound'}};
  const pod = {apiVersion: 'v1', kind: 'Pod', metadata: {...metadata('worker', 'pod-uid'),
    labels: {'cma/runtime-name': 'member'}, ownerReferences: [{apiVersion: sandbox.apiVersion,
      kind: 'Sandbox', name: 'member', uid: 'member-uid', controller: true}]},
  spec: structuredClone(sandbox.spec.podTemplate.spec), status: {phase: 'Running'}};
  const pods = {apiVersion: 'v1', kind: 'PodList', metadata: {resourceVersion: '20'},
    items: mode === 'Running' ? [pod] : []};
  const routes = {
    '/api/v1/namespaces/fixture': namespace,
    '/apis/agents.x-k8s.io/v1beta1/namespaces/fixture/sandboxes/member': sandbox,
    '/api/v1/namespaces/fixture/persistentvolumeclaims/home': pvc,
    '/api/v1/namespaces/fixture/pods?limit=128': pods,
  };
  return {binding, namespace, sandbox, pvc, pod, pods, routes};
}

export async function server(t, data = fixture(), respond) {
  const requests = [];
  const http = createServer((req, res) => {
    requests.push({method: req.method, url: req.url});
    if (respond?.(req, res, requests, data)) return;
    const value = data.routes[req.url];
    res.writeHead(value ? 200 : 404, {'Content-Type': 'application/json'});
    res.end(JSON.stringify(value ?? {code: 'not_found'}));
  });
  http.listen(0, '127.0.0.1');
  await once(http, 'listening');
  t.after(() => new Promise((resolve, reject) => {
    http.closeAllConnections();
    http.close(error => error ? reject(error) : resolve());
  }));
  return {origin: `http://127.0.0.1:${http.address().port}`, data, requests};
}
