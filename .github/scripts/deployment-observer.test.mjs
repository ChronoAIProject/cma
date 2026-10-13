import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import { createServer } from 'node:http';
import { chmodSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import test from 'node:test';
import { observe, projection, reference, registryImage, runningReference, securePath, sourceKeysProjection, validateConfig, validateRequest, verifyAuthorityFiles } from './deployment-observer.mjs';

const source = 'https://github.com/ChronoAIProject/cma';
const commit = 'a'.repeat(40);
const nyxidCommit = 'b'.repeat(40);
const sha = (bytes) => `sha256:${createHash('sha256').update(bytes).digest('hex')}`;
const raw = (value) => Buffer.from(JSON.stringify(value));
const manifestType = 'application/vnd.oci.image.manifest.v1+json';
const indexType = 'application/vnd.oci.image.index.v1+json';
const configType = 'application/vnd.oci.image.config.v1+json';
const platform = { os: 'linux', architecture: 'amd64' };

function fixture() {
  const config = {
    schema: 'cma-deployment-observer-config-v1',
    tools: { kubectl: { path: '/protected/kubectl', sha256: '1'.repeat(64) }, skopeo: { path: '/protected/skopeo', sha256: '2'.repeat(64) } },
    kubeconfig: '/protected/kubeconfig', registryAuthFile: '/protected/registry-auth.json',
    context: 'reviewed-cluster', namespace: 'cma',
    api: { deployment: 'cma-agent-manager', container: 'api', service: 'cma-agent-manager', servicePort: 80 },
    web: { deployment: 'cma-web', container: 'web', service: 'cma-web', servicePort: 80 },
    configMap: { name: 'cma-agent-manager', runtimeKey: 'CHRONO_SANDBOX_IMAGE', originKey: 'CHRONO_SERVER_PUBLIC_URL' },
    route: { name: 'cma-web', parent: { name: 'gateway', namespace: 'gateway-system', sectionName: 'https' } },
    runtimePlatform: platform,
    nyxid: { context: 'reviewed-cluster', namespace: 'identity', deployment: 'nyxid-api', container: 'nyxid', source: 'https://github.com/example/identity' },
  };
  const registry = new Map();
  const images = {};
  for (const component of ['api', 'web', 'runtime', 'nyxid']) {
    const repository = `registry.example.test:5443/team/${component}`;
    const configuration = raw({ ...platform, config: { Labels: { 'org.opencontainers.image.source': component === 'nyxid' ? config.nyxid.source : source, 'org.opencontainers.image.revision': component === 'nyxid' ? nyxidCommit : commit, ...(component === 'runtime' ? { 'dev.chrono.cma.runtime-contract': '3' } : {}) } } });
    const configDigest = sha(configuration);
    const manifest = raw({ schemaVersion: 2, mediaType: manifestType, config: { mediaType: configType, digest: configDigest, size: configuration.length }, layers: [] });
    const platformDigest = sha(manifest);
    const root = component === 'web' ? manifest : raw({ schemaVersion: 2, mediaType: indexType, manifests: [{ mediaType: manifestType, digest: platformDigest, size: manifest.length, platform }] });
    const rootDigest = sha(root);
    const reference = `${repository}@${rootDigest}`;
    registry.set(`${repository}@${rootDigest}`, root);
    registry.set(`${repository}@${platformDigest}`, manifest);
    registry.set(`${repository}@${platformDigest}:config`, configuration);
    images[component] = { reference, repository, rootDigest, platformDigest, configDigest, runningImageID: `docker-pullable://${repository}@${platformDigest}` };
  }
  const request = { schema: 'cma-staging-attestation-request-v1', nonce: '12345678-1234-4234-8234-123456789abc', expected: { origin: 'https://cma.example.test', commit, nyxidCommit, images: Object.fromEntries(['api', 'web', 'runtime'].map((name) => [name, images[name].reference])) } };
  const resources = new Map();
  const add = (namespace, kind, name, fields) => resources.set(`${namespace}/${kind}/${name ?? ''}`, structuredClone(fields));
  const condition = (type) => ({ type, status: 'True' });
  const owner = (kind, name, uid) => ({ apiVersion: 'apps/v1', kind, name, uid, controller: true });
  for (const component of ['api', 'web', 'nyxid']) {
    const namespace = component === 'nyxid' ? 'identity' : 'cma';
    const target = config[component];
    const label = { app: target.deployment };
    add(namespace, 'deployment', target.deployment, {
      'metadata.name': [target.deployment], 'metadata.namespace': [namespace], 'metadata.uid': [`${component}-deployment`], 'metadata.generation': [2], 'metadata.resourceVersion': ['20'],
      'spec.replicas': [1], 'spec.selector': [{ matchLabels: label }], 'spec.template.metadata.labels': [label],
      status: [{ observedGeneration: 2, replicas: 1, readyReplicas: 1, updatedReplicas: 1, availableReplicas: 1, conditions: [condition('Available'), condition('Progressing')] }],
      'spec.template.spec.containers[*].name': [target.container],
      'spec.template.spec.containers[*].image': [images[component].reference.replace('@', ':reviewed@')],
      [`spec.template.spec.containers[?(@.name=="${target.container}")].ports[*]`]: [{ name: 'http', containerPort: 8080 }],
      [`spec.template.spec.containers[?(@.name=="${target.container}")].envFrom[*]`]: component === 'api' ? [{ configMapRef: { name: 'cma-agent-manager' } }, { secretRef: { name: 'cma-agent-manager' } }] : [],
    });
    add(namespace, 'pods', `app=${target.deployment}`, { 'items[*].metadata.name': [`${component}-pod`] });
    add(namespace, 'pod', `${component}-pod`, {
      'metadata.name': [`${component}-pod`], 'metadata.namespace': [namespace], 'metadata.uid': [`${component}-pod-uid`], 'metadata.resourceVersion': ['21'],
      'metadata.ownerReferences': [[owner('ReplicaSet', `${component}-rs`, `${component}-rs-uid`)]], 'metadata.labels': [label], 'spec.nodeName': ['worker'],
      status: [{ phase: 'Running', conditions: [condition('Ready'), condition('ContainersReady')], containerStatuses: [{ name: target.container, ready: true, started: true, state: { running: {} }, imageID: images[component].runningImageID }] }],
      'spec.containers[*].name': [target.container], 'spec.containers[*].image': [images[component].reference.replace('@', ':reviewed@')],
    });
    add(namespace, 'replicaset', `${component}-rs`, { 'metadata.name': [`${component}-rs`], 'metadata.namespace': [namespace], 'metadata.uid': [`${component}-rs-uid`], 'metadata.ownerReferences': [[owner('Deployment', target.deployment, `${component}-deployment`)]] });
    add(namespace, 'node', 'worker', { 'metadata.name': ['worker'], 'status.nodeInfo.operatingSystem': ['linux'], 'status.nodeInfo.architecture': ['amd64'] });
    if (component !== 'nyxid') add(namespace, 'service', target.service, { 'metadata.name': [target.service], 'metadata.namespace': [namespace], 'metadata.uid': [`${component}-service`], 'metadata.resourceVersion': ['25'], 'spec.selector': [label], 'spec.ports': [[{ port: 80, targetPort: 'http', protocol: 'TCP' }]], 'spec.type': ['ClusterIP'] });
  }
  add('cma', 'configmap', 'cma-agent-manager', { 'metadata.name': ['cma-agent-manager'], 'metadata.namespace': ['cma'], 'metadata.uid': ['config-uid'], 'metadata.resourceVersion': ['22'], 'data.CHRONO_SANDBOX_IMAGE': [images.runtime.reference.replace('@', ':reviewed@')], 'data.CHRONO_SERVER_PUBLIC_URL': [request.expected.origin] });
  add('cma', 'secret', 'cma-agent-manager', { sourceKeys: { name: 'cma-agent-manager', namespace: 'cma', uid: 'secret-uid', resourceVersion: '24', keys: ['CHRONO_MONGODB_URI', 'CHRONO_SESSION_SIGNING_KEY'], deleting: false } });
  const match = (type, value) => ({ path: { type, value } });
  const backend = (name) => ({ name, kind: 'Service', group: '', port: 80, weight: 1 });
  add('cma', 'httproute', 'cma-web', {
    'metadata.name': ['cma-web'], 'metadata.namespace': ['cma'], 'metadata.uid': ['route-uid'], 'metadata.generation': [3], 'metadata.resourceVersion': ['23'],
    'spec.hostnames': [['cma.example.test']], 'spec.parentRefs': [[config.route.parent]],
    'spec.rules': [[{ matches: [match('PathPrefix', '/api'), match('PathPrefix', '/auth'), match('Exact', '/openapi.json'), match('Exact', '/.well-known/agent-card.json')], backendRefs: [backend('cma-agent-manager')] }, { matches: [match('PathPrefix', '/')], backendRefs: [backend('cma-web')] }]],
    'status.parents': [[{ parentRef: config.route.parent, conditions: ['Accepted', 'ResolvedRefs'].map((type) => ({ ...condition(type), observedGeneration: 3 })) }]],
  });
  const calls = [];
  const command = (tool, args) => {
    calls.push({ tool, args });
    if (tool === config.tools.skopeo.path) {
      assert.equal(args[1], 'inspect');
      assert.ok(args.includes('--raw') && args.includes('--tls-verify=true'));
      assert.equal(args[args.indexOf('--authfile') + 1], config.registryAuthFile);
      const key = args.at(-1).replace('docker://', '') + (args.includes('--config') ? ':config' : '');
      assert.ok(registry.has(key), 'only known immutable image documents may be read');
      return registry.get(key);
    }
    assert.equal(tool, config.tools.kubectl.path);
    assert.equal(args[args.indexOf('--kubeconfig') + 1], config.kubeconfig);
    assert.equal(args[args.indexOf('--context') + 1], config.context);
    const namespace = args[args.indexOf('--namespace') + 1];
    const at = args.indexOf('get'); assert.equal(at, 7);
    const kind = args[at + 1]; assert.ok(['deployment', 'pods', 'pod', 'replicaset', 'node', 'service', 'httproute', 'configmap', 'secret'].includes(kind));
    const name = kind === 'pods' ? args[args.indexOf('--selector') + 1] : args[at + 2];
    const fields = resources.get(`${namespace}/${kind}/${name}`); assert.ok(fields);
    if (args.at(-1) === sourceKeysProjection) {
      assert.ok(['configmap', 'secret'].includes(kind)); assert.ok(fields.sourceKeys);
      return raw(fields.sourceKeys);
    }
    assert.notEqual(kind, 'secret', 'Secret values must never be projected');
    const union = args.at(-1).match(/^jsonpath-as-json=\{\['(.*)'\]\}$/);
    const single = args.at(-1).match(/^jsonpath-as-json=\{\.(.*)\}$/); assert.ok(union || single);
    const paths = union ? union[1].split("','") : [single[1]];
    assert.ok(paths.every((path) => !['data', 'spec', 'metadata', 'spec.containers'].includes(path)));
    assert.ok(!paths.some((path) => path.includes('.env[') && !path.endsWith('.name')));
    return raw(paths.flatMap((path) => fields[path] ?? []));
  };
  return { config, request, registry, images, resources, calls, command };
}

test('independent observation binds ready Kubernetes targets to raw root, child and config bytes', () => {
  const f = fixture(); const receipt = observe(f.config, f.request, f.command, () => new Date('2026-09-07T00:00:00Z'));
  assert.equal(receipt.nonce, f.request.nonce); assert.equal(receipt.origin, f.request.expected.origin);
  assert.equal(receipt.commit, commit); assert.equal(receipt.nyxidCommit, nyxidCommit);
  assert.equal(receipt.runtimePin, f.images.runtime.reference);
  for (const component of ['api', 'web', 'runtime']) {
    for (const field of ['reference', 'rootDigest', 'platformDigest', 'configDigest']) assert.equal(receipt.images[component][field], f.images[component][field]);
    assert.equal(receipt.images[component].source, source); assert.equal(receipt.images[component].revision, commit);
  }
  assert.notEqual(receipt.images.api.rootDigest, receipt.images.api.platformDigest);
  assert.notEqual(receipt.images.api.platformDigest, receipt.images.api.configDigest);
  assert.equal(receipt.images.web.rootDigest, receipt.images.web.platformDigest);
  assert.equal(receipt.images.api.runningImageID, f.images.api.runningImageID);
  assert.ok(!Object.hasOwn(receipt.images.runtime, 'runningImageID'));
  assert.ok(f.calls.filter((call) => call.tool === f.config.tools.skopeo.path).every((call) => call.args.at(-1).includes('@sha256:')));
});

test('immutable reference parsing retains registry ports and removes only the tag', () => {
  const digest = `sha256:${'a'.repeat(64)}`;
  assert.equal(reference(`registry.example.test:5443/team/api:version@${digest}`).canonical, `registry.example.test:5443/team/api@${digest}`);
  assert.equal(runningReference(`docker-pullable://registry.example.test/team/api@${digest}`).digest, digest);
  for (const value of [`docker://${digest}`, `containerd://${digest}`, 'registry.example.test/api:latest', `https://registry.example.test/api@${digest}`, `registry.example.test/a/../api@${digest}`]) assert.throws(() => runningReference(value));
});

const mutations = {
  'unobserved deployment generation': (f) => { f.resources.get('cma/deployment/cma-agent-manager').status[0].observedGeneration = 1; },
  'unready deployment replicas': (f) => { f.resources.get('cma/deployment/cma-web').status[0].readyReplicas = 0; },
  'terminating pod': (f) => { f.resources.get('cma/pod/api-pod')['metadata.deletionTimestamp'] = ['now']; },
  'extra selected pod': (f) => { f.resources.get('cma/pods/app=cma-agent-manager')['items[*].metadata.name'].push('old-pod'); },
  'foreign replica owner': (f) => { f.resources.get('cma/replicaset/api-rs')['metadata.ownerReferences'][0][0].uid = 'foreign'; },
  'unready container': (f) => { f.resources.get('cma/pod/api-pod').status[0].containerStatuses[0].ready = false; },
  'config digest used as imageID': (f) => { f.resources.get('cma/pod/api-pod').status[0].containerStatuses[0].imageID = `docker-pullable://${f.images.api.repository}@${f.images.api.configDigest}`; },
  'index root used as running child': (f) => { f.resources.get('cma/pod/api-pod').status[0].containerStatuses[0].imageID = `docker-pullable://${f.images.api.reference}`; },
  'runtime pin drift': (f) => { f.resources.get('cma/configmap/cma-agent-manager')['data.CHRONO_SANDBOX_IMAGE'] = [f.images.api.reference]; },
  'origin drift': (f) => { f.resources.get('cma/configmap/cma-agent-manager')['data.CHRONO_SERVER_PUBLIC_URL'] = ['https://other.example.test']; },
  'explicit runtime env override': (f) => { f.resources.get('cma/deployment/cma-agent-manager')['spec.template.spec.containers[?(@.name=="api")].env[*].name'] = ['CHRONO_SANDBOX_IMAGE']; },
  'unbound API ConfigMap': (f) => { f.resources.get('cma/deployment/cma-agent-manager')['spec.template.spec.containers[?(@.name=="api")].envFrom[*]'] = [{ configMapRef: { name: 'other' } }]; },
  'stale route readiness': (f) => { f.resources.get('cma/httproute/cma-web')['status.parents'][0][0].conditions[0].observedGeneration = 2; },
  'wrong route backend': (f) => { f.resources.get('cma/httproute/cma-web')['spec.rules'][0][0].backendRefs[0].name = 'cma-web'; },
  'wildcard route hostname': (f) => { f.resources.get('cma/httproute/cma-web')['spec.hostnames'] = [['*.example.test']]; },
  'wrong service selector': (f) => { f.resources.get('cma/service/cma-agent-manager')['spec.selector'][0].app = 'other'; },
  'wrong service target port': (f) => { f.resources.get('cma/service/cma-agent-manager')['spec.ports'][0][0].targetPort = 9000; },
  'missing independent NyxID scope': (f) => { f.config.nyxid = null; },
  'declared NyxID revision mismatch': (f) => { f.request.expected.nyxidCommit = 'c'.repeat(40); },
};
const refusalCodes = [
  'deployment_generation_unobserved', 'deployment_not_settled', 'pod_identity_or_deletion',
  'pod_population_not_settled', 'pod_owner_uid_mismatch', 'container_not_ready',
  'running_image_is_not_platform_manifest', 'running_image_is_not_platform_manifest',
  'configured_runtime_or_origin_mismatch', 'configured_runtime_or_origin_mismatch',
  'api_configmap_explicitly_overridden', 'api_configmap_not_bound', 'stale_route_status',
  'route_path_mapping_mismatch', 'route_origin_mismatch', 'service_selector_mismatch',
  'service_target_port_mismatch', 'nyxid_scope_incomplete', 'registry_source_or_revision_mismatch',
];
assert.equal(Object.keys(mutations).length, refusalCodes.length);
for (const [index, [name, mutate]] of Object.entries(mutations).entries()) test(`refuses ${name}`, () => {
  const f = fixture(); mutate(f); assert.throws(() => observe(f.config, f.request, f.command), new RegExp(`^Error: ${refusalCodes[index]}$`));
});

function broadServiceFixture() {
  const f = fixture();
  const dep = f.resources.get('cma/deployment/cma-agent-manager');
  dep['spec.selector'][0].matchLabels.revision = 'new';
  dep['spec.template.metadata.labels'][0].revision = 'new';
  f.resources.get('cma/pod/api-pod')['metadata.labels'][0].revision = 'new';
  f.resources.set('cma/pods/app=cma-agent-manager,revision=new', { 'items[*].metadata.name': ['api-pod'] });
  return f;
}

test('a broader Service selector is allowed only when its entire population is attested', () => {
  const f = broadServiceFixture(); observe(f.config, f.request, f.command);
  f.resources.get('cma/pods/app=cma-agent-manager')['items[*].metadata.name'].push('old-pod');
  assert.throws(() => observe(f.config, f.request, f.command), /service_pod_population_mismatch/);
});

test('Service population is checked again after registry reads', () => {
  const f = broadServiceFixture(); let changed = false;
  assert.throws(() => observe(f.config, f.request, (tool, args) => {
    if (tool === f.config.tools.skopeo.path && !changed) {
      changed = true; f.resources.get('cma/pods/app=cma-agent-manager')['items[*].metadata.name'].push('old-pod');
    }
    return f.command(tool, args);
  }), /service_pod_population_mismatch/);
});

test('a same-name replacement Pod cannot satisfy the Service identity check', () => {
  const f = fixture();
  assert.throws(() => observe(f.config, f.request, (tool, args) => {
    const bytes = f.command(tool, args);
    if (args.at(-1) === projection(['metadata.name', 'metadata.namespace', 'metadata.uid', 'metadata.resourceVersion', 'metadata.labels', 'metadata.deletionTimestamp'])) {
      const identity = JSON.parse(bytes); identity[2] = 'replacement-uid'; return raw(identity);
    }
    return bytes;
  }), /service_pod_identity_mismatch/);
});

const environmentSources = (f) => f.resources.get('cma/deployment/cma-agent-manager')['spec.template.spec.containers[?(@.name=="api")].envFrom[*]'];
function addSource(f, kind, name, keys, prefix = '') {
  f.resources.set(`cma/${kind}/${name}`, { sourceKeys: { name, namespace: 'cma', uid: `${name}-uid`, resourceVersion: '30', keys, deleting: false } });
  environmentSources(f).push({ ...(prefix ? { prefix } : {}), [kind === 'secret' ? 'secretRef' : 'configMapRef']: { name } });
}

test('the required ConfigMap cannot be prefixed or ambiguous', () => {
  const f = fixture(); environmentSources(f)[0].prefix = 'OTHER_';
  assert.throws(() => observe(f.config, f.request, f.command), /api_configmap_prefixed/);
  environmentSources(f)[0].prefix = ''; environmentSources(f).push({ configMapRef: { name: 'cma-agent-manager' } });
  assert.throws(() => observe(f.config, f.request, f.command), /api_configmap_not_bound/);
});

for (const kind of ['configmap', 'secret']) for (const key of ['CHRONO_SANDBOX_IMAGE', 'CHRONO_SERVER_PUBLIC_URL']) test(`later ${kind} cannot override ${key}`, () => {
  const f = fixture(); addSource(f, kind, 'override', [key]);
  assert.throws(() => observe(f.config, f.request, f.command), /api_configmap_shadowed/);
});

test('source prefix and key are combined before checking protected environment names', () => {
  const f = fixture(); addSource(f, 'secret', 'override', ['SANDBOX_IMAGE'], 'CHRONO_');
  assert.throws(() => observe(f.config, f.request, f.command), /api_configmap_shadowed/);
  environmentSources(f).at(-1).prefix = 'UNRELATED_';
  observe(f.config, f.request, f.command);
});

test('an earlier conflicting source does not override the later required ConfigMap', () => {
  const f = fixture(); addSource(f, 'configmap', 'earlier', ['CHRONO_SANDBOX_IMAGE']);
  environmentSources(f).unshift(environmentSources(f).pop());
  observe(f.config, f.request, f.command);
  assert.ok(!f.calls.some((call) => call.args.includes('earlier')), 'an earlier overridden source needs no key inspection');
});

test('the current unprefixed ConfigMap then unrelated Secret layout remains valid', () => {
  const f = fixture(); const receipt = observe(f.config, f.request, f.command);
  const calls = f.calls.filter((call) => call.args.includes('secret'));
  assert.equal(calls.length, 2);
  assert.ok(calls.every((call) => call.args.at(-1) === sourceKeysProjection));
  assert.ok(!JSON.stringify(receipt).includes('CHRONO_SESSION_SIGNING_KEY'));
});

test('later source key and identity changes are checked in both observations', () => {
  for (const field of ['keys', 'resourceVersion']) {
    const f = fixture(); let changed = false;
    assert.throws(() => observe(f.config, f.request, (tool, args) => {
      if (tool === f.config.tools.skopeo.path && !changed) {
        changed = true;
        const source = f.resources.get('cma/secret/cma-agent-manager').sourceKeys;
        if (field === 'keys') source.keys.push('CHRONO_SANDBOX_IMAGE'); else source.resourceVersion = '31';
      }
      return f.command(tool, args);
    }), field === 'keys' ? /api_configmap_shadowed/ : /deployment_changed_during_observation/);
  }
});

test('missing, inaccessible or malformed later source evidence fails closed', () => {
  const missing = fixture(); missing.resources.delete('cma/secret/cma-agent-manager');
  assert.throws(() => observe(missing.config, missing.request, missing.command));
  const wrong = fixture(); wrong.resources.get('cma/secret/cma-agent-manager').sourceKeys.namespace = 'other';
  assert.throws(() => observe(wrong.config, wrong.request, wrong.command), /environment_source_identity_incomplete/);
  const value = fixture(); value.resources.get('cma/secret/cma-agent-manager').sourceKeys.extraValue = 'untrusted-data';
  assert.throws(() => observe(value.config, value.request, value.command), /invalid_schema/);
  const inaccessible = fixture();
  assert.throws(() => observe(inaccessible.config, inaccessible.request, (tool, args) => {
    if (args.at(-1) === sourceKeysProjection) throw new Error('source-inaccessible');
    return inaccessible.command(tool, args);
  }), /source-inaccessible/);
});

test('an index with duplicate running-platform children is ambiguous even when both digests match', () => {
  const f = fixture(); const root = JSON.parse(f.registry.get(f.images.api.reference));
  root.manifests.push(structuredClone(root.manifests[0]));
  const bytes = raw(root); const ref = `${f.images.api.repository}@${sha(bytes)}`; f.registry.set(ref, bytes);
  assert.throws(() => registryImage(f.config, f.command, ref, platform, source, commit), /ambiguous_registry_platform/);
});

test('the observer refuses observations beyond its bounded time window', () => {
  const f = fixture(); let now = 0;
  assert.throws(() => observe(f.config, f.request, f.command, () => new Date(now++ * 240001)), /observation_deadline/);
});

test('registry byte verification refuses reserialized or substituted config documents', () => {
  const f = fixture(); const key = `${f.images.api.repository}@${f.images.api.platformDigest}:config`;
  f.registry.set(key, Buffer.concat([f.registry.get(key), Buffer.from('\n')]));
  assert.throws(() => registryImage(f.config, f.command, f.images.api.reference, platform, source, commit), /registry_content_digest_mismatch/);
});

test('registry labels and contract are checked against bytes, not fixture declarations', () => {
  const f = fixture();
  assert.throws(() => registryImage(f.config, f.command, f.images.api.reference, platform, 'https://other.example.test/repo', commit), /registry_source_or_revision_mismatch/);
  assert.throws(() => registryImage(f.config, f.command, f.images.api.reference, platform, source, commit, true), /runtime_contract_mismatch/);
  assert.throws(() => registryImage(f.config, f.command, f.images.api.reference, { os: 'linux', architecture: 'arm64' }, source, commit), /ambiguous_registry_platform/);
});

test('a rollout occurring during registry reads cannot produce a passing receipt', () => {
  const f = fixture(); let changed = false;
  assert.throws(() => observe(f.config, f.request, (tool, args) => {
    if (tool === f.config.tools.skopeo.path && !changed) { changed = true; f.resources.get('cma/deployment/cma-agent-manager')['metadata.resourceVersion'] = ['new']; }
    return f.command(tool, args);
  }), /deployment_changed_during_observation/);
});

test('request and config schemas refuse unknown fields, unpinned images, tool substitution and wrong origins', () => {
  const f = fixture(); validateRequest(f.request); validateConfig(f.config);
  assert.throws(() => validateRequest({ ...f.request, passing: true }));
  assert.throws(() => validateRequest({ ...f.request, expected: { ...f.request.expected, origin: `${f.request.expected.origin}/path` } }));
  assert.throws(() => validateConfig({ ...f.config, tools: { ...f.config.tools, kubectl: { path: 'kubectl', sha256: 'a'.repeat(64) } } }));
  assert.throws(() => validateConfig({ ...f.config, runtimePlatform: { os: 'linux', architecture: 'arm64' } }));
});

test('authority files must be private real files and tools must match reviewed bytes', () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'cma-observer-permissions-')));
  try {
    const privatePath = join(directory, 'authority'); writeFileSync(privatePath, '{}', { mode: 0o600 });
    securePath(privatePath, 'private');
    const link = join(directory, 'link'); symlinkSync(privatePath, link); assert.throws(() => securePath(link, 'private'), /symlink_refused/);
    chmodSync(privatePath, 0o644); assert.throws(() => securePath(privatePath, 'private'), /private_file_permissions_required/); chmodSync(privatePath, 0o600);
    const tool = join(directory, 'tool'); writeFileSync(tool, '#!/bin/sh\nexit 0\n', { mode: 0o700 });
    const config = fixture().config; config.kubeconfig = privatePath; config.registryAuthFile = privatePath;
    config.tools = { kubectl: { path: tool, sha256: sha(Buffer.from('#!/bin/sh\nexit 0\n')).slice(7) }, skopeo: { path: tool, sha256: sha(Buffer.from('#!/bin/sh\nexit 0\n')).slice(7) } };
    verifyAuthorityFiles(config); config.tools.skopeo.sha256 = 'a'.repeat(64); assert.throws(() => verifyAuthorityFiles(config), /tool_digest_mismatch/);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test('real kubectl projection syntax works against an isolated loopback API', { skip: !process.env.CMA_TEST_KUBECTL }, async () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'cma-observer-kubectl-')));
  const deployment = { apiVersion: 'apps/v1', kind: 'Deployment', metadata: { name: 'sample', namespace: 'test' }, spec: { template: { spec: { containers: [{ name: 'api', image: 'example.test/api', ports: [{ name: 'http', containerPort: 8080 }] }] } } } };
  const secret = { apiVersion: 'v1', kind: 'Secret', metadata: { name: 'sample', namespace: 'test', uid: 'source-uid', resourceVersion: '5' }, data: { VISIBLE_KEY: Buffer.from('must-not-be-output').toString('base64'), ANOTHER_KEY: Buffer.from('also-not-output').toString('base64') } };
  const server = createServer((request, response) => {
    const path = new URL(request.url, 'http://localhost').pathname;
    const group = { name: 'apps', versions: [{ groupVersion: 'apps/v1', version: 'v1' }], preferredVersion: { groupVersion: 'apps/v1', version: 'v1' } };
    const responses = {
      '/api': { kind: 'APIVersions', apiVersion: 'v1', versions: ['v1'] },
      '/apis': { kind: 'APIGroupList', apiVersion: 'v1', groups: [group] },
      '/apis/apps/v1': { kind: 'APIResourceList', apiVersion: 'v1', groupVersion: 'apps/v1', resources: [{ name: 'deployments', singularName: 'deployment', namespaced: true, kind: 'Deployment', verbs: ['get'] }] },
      '/api/v1': { kind: 'APIResourceList', apiVersion: 'v1', groupVersion: 'v1', resources: [{ name: 'secrets', singularName: 'secret', namespaced: true, kind: 'Secret', verbs: ['get'] }] },
      '/apis/apps/v1/namespaces/test/deployments/sample': deployment,
      '/api/v1/namespaces/test/secrets/sample': secret,
    };
    response.writeHead(responses[path] ? 200 : 404, { 'content-type': 'application/json' });
    response.end(JSON.stringify(responses[path] ?? { kind: 'Status', status: 'Failure' }));
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  try {
    for (const [paths, expected] of [[['metadata.name', 'metadata.namespace', 'metadata.deletionTimestamp'], ['sample', 'test']], [['spec.template.spec.containers[?(@.name=="api")].ports[*]'], [{ name: 'http', containerPort: 8080 }]], [['spec.template.spec.containers[*].name'], ['api']]]) {
      const { stdout } = await promisify(execFile)(process.env.CMA_TEST_KUBECTL, ['--kubeconfig=/dev/null', `--server=http://127.0.0.1:${server.address().port}`, `--cache-dir=${directory}`, '--namespace=test', 'get', 'deployment', 'sample', '-o', projection(paths)], { timeout: 10000, maxBuffer: 1024 * 1024, env: { PATH: '/usr/local/bin:/usr/bin:/bin' } });
      assert.deepEqual(JSON.parse(stdout), expected);
    }
    for (const expectedKeys of [['ANOTHER_KEY', 'VISIBLE_KEY'], []]) {
      if (!expectedKeys.length) secret.data = {};
      const { stdout } = await promisify(execFile)(process.env.CMA_TEST_KUBECTL, ['--kubeconfig=/dev/null', `--server=http://127.0.0.1:${server.address().port}`, `--cache-dir=${directory}`, '--namespace=test', 'get', 'secret', 'sample', '-o', sourceKeysProjection], { timeout: 10000, maxBuffer: 1024 * 1024, env: { PATH: '/usr/local/bin:/usr/bin:/bin' } });
      assert.deepEqual(JSON.parse(stdout), { name: 'sample', namespace: 'test', uid: 'source-uid', resourceVersion: '5', keys: expectedKeys, deleting: false });
      for (const value of Object.values(secret.data)) assert.ok(!stdout.includes(value), 'only key names may leave kubectl');
    }
  } finally {
    await new Promise((resolve) => server.close(resolve));
    rmSync(directory, { recursive: true, force: true });
  }
});
