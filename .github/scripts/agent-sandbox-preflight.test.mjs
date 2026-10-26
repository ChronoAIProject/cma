import assert from 'node:assert/strict';
import test from 'node:test';
import { homePolicy, nativeResources, preflight, productionContext, productionRuntimeContext, validateCrd, validateSeed } from './agent-sandbox-preflight.mjs';

function crd([plural, group, fields]) {
  const spec = Object.fromEntries(fields.map((field) => [field, { type: 'object' }]));
  if (plural === 'sandboxes') spec.operatingMode = { enum: ['Running', 'Suspended'] };
  if (plural === 'sandboxtemplates') spec.volumeClaimTemplatesPolicy = { enum: ['Disallowed'] };
  if (plural === 'sandboxwarmpools') spec.updateStrategy = { properties: { type: { enum: ['OnReplenish'] } } };
  return {
    metadata: { name: `${plural}.${group}` },
    spec: { group, scope: 'Namespaced', names: { plural }, versions: [{
      name: 'v1beta1', served: true, schema: { openAPIV3Schema: { properties: {
        spec: { properties: spec }, status: { properties: {
          readyReplicas: {}, observedGeneration: {}, sandbox: { properties: { name: {} } },
        } },
      } } },
    }] },
    status: { conditions: [{ type: 'Established', status: 'True' }] },
  };
}
const seed = () => ({ metadata: { uid: 'original-seed', annotations: { 'cma/home-seed-format': '1' } }, status: { readyToUse: true, boundVolumeSnapshotContentName: 'seed-content' } });

test('served native APIs require the actual CMA schema and lifecycle fields', () => {
  for (const resource of nativeResources) {
    validateCrd(crd(resource), resource);
    for (const mutate of [
      (value) => { value.spec.versions[0].name = 'v1alpha1'; },
      (value) => { value.spec.versions[0].served = false; },
      (value) => { value.status.conditions[0].status = 'False'; },
      (value) => { value.spec.scope = 'Cluster'; },
      (value) => { delete value.spec.versions[0].schema.openAPIV3Schema.properties.spec.properties[resource[2][0]]; },
    ]) {
      const changed = crd(resource);
      mutate(changed);
      assert.throws(() => validateCrd(changed, resource));
    }
  }
});

test('home seed preflight rejects replacement, deletion, unready and missing initialization', () => {
  validateSeed(seed(), 'original-seed');
  for (const mutate of [
    (value) => { value.metadata.uid = 'replaced'; },
    (value) => { value.metadata.deletionTimestamp = 'now'; },
    (value) => { value.status.readyToUse = false; },
    (value) => { delete value.status.boundVolumeSnapshotContentName; },
    (value) => { delete value.metadata.annotations; },
  ]) {
    const changed = seed();
    mutate(changed);
    assert.throws(() => validateSeed(changed, 'original-seed'));
  }
});

const legacyConfig = {
  CHRONO_SANDBOX_NAMESPACE: 'chrono-bot',
  CHRONO_SANDBOX_HOME_TEMPLATE: 'home-seed', CHRONO_SANDBOX_HOME_TEMPLATE_UID: 'original-seed',
  CHRONO_SANDBOX_HOME_STORAGE_CLASS: 'cma-home-v1', CHRONO_SANDBOX_HOME_CAPACITY_GIB: '64',
  CHRONO_AGENT_SANDBOX_SERVICE_ACCOUNT: 'cma-runtime',
};
const remoteOptions = {
  runtimeContext: productionRuntimeContext, runtimeNamespace: 'native-cma',
  runtimePrincipal: 'system:serviceaccount:native-cma:cma-controller',
};
const remoteConfig = {
  ...legacyConfig,
  CHRONO_AGENT_SANDBOX_NAMESPACE: remoteOptions.runtimeNamespace,
  CHRONO_AGENT_SANDBOX_API_SERVER: 'https://runtime.invalid',
  CHRONO_AGENT_SANDBOX_CA_FILE: '/var/run/native/ca.crt',
  CHRONO_AGENT_SANDBOX_TOKEN_FILE: '/var/run/native/token',
  CHRONO_AGENT_SANDBOX_HOME_TEMPLATE: 'remote-seed', CHRONO_AGENT_SANDBOX_HOME_TEMPLATE_UID: 'original-seed',
  CHRONO_AGENT_SANDBOX_HOME_STORAGE_CLASS: 'remote-disk', CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB: '32',
};

function fixture(fail = () => false, config = legacyConfig, options = {}, deploymentEnv = [], extraConfigMaps = {}) {
  const calls = [];
  const reviews = [];
  return { calls, reviews, command(args, input) {
    calls.push(args);
    const review = input === undefined ? undefined : JSON.parse(input);
    if (review) reviews.push(review.spec.resourceAttributes);
    if (fail(args, review?.spec.resourceAttributes)) throw new Error('sensitive raw cluster detail');
    if (args[2] === 'config') {
      assert.deepEqual(args, ['--context', options.runtimeContext, 'config', 'view', '--minify', '-o', 'jsonpath={.clusters[0].cluster.server}']);
      return 'https://runtime.invalid';
    }
    const [operation, kind, name] = args.slice(5);
    const control = operation === 'get' && ['configmap', 'deployment'].includes(kind);
    const context = control ? productionContext : options.runtimeContext ?? productionContext;
    const namespace = control ? 'chrono-bot' : options.runtimeNamespace ?? 'chrono-bot';
    assert.deepEqual(args.slice(0, 5), ['--context', context, '--namespace', namespace, '--request-timeout=10s']);
    if (operation === 'create') {
      assert.deepEqual(args.slice(5), ['create', '--raw', '/apis/authorization.k8s.io/v1/selfsubjectaccessreviews',
        '-f', '-', '--as', options.runtimePrincipal ?? 'system:serviceaccount:chrono-bot:chrono-bot-ksa']);
      assert.equal(review.apiVersion, 'authorization.k8s.io/v1');
      assert.equal(review.kind, 'SelfSubjectAccessReview');
      assert.deepEqual(Object.keys(review.spec), ['resourceAttributes']);
      assert.equal(review.spec.resourceAttributes.namespace, namespace);
      return JSON.stringify({ ...review, status: { allowed: true } });
    }
    assert.equal(input, undefined);
    assert.equal(operation, 'get');
    assert.deepEqual(args.slice(-2), ['-o', 'json']);
    if (kind === 'customresourcedefinition') return JSON.stringify(crd(nativeResources.find(([plural, group]) => name === `${plural}.${group}`)));
    if (kind === 'configmap') {
      if (name === 'cma-agent-manager') return JSON.stringify({ data: config });
      if (extraConfigMaps[name]) return JSON.stringify({ data: extraConfigMaps[name] });
      assert.ok(args.includes('--ignore-not-found'));
      return '';
    }
    if (kind === 'deployment') return JSON.stringify({ spec: { template: { spec: { containers: [{ name: 'api', env: deploymentEnv }] } } } });
    if (kind === 'serviceaccount') return JSON.stringify({ metadata: { name, namespace }, automountServiceAccountToken: false });
    if (kind === 'runtimeclass.node.k8s.io') return JSON.stringify({ metadata: { name }, handler: 'test-handler' });
    if (kind === 'storageclass') return JSON.stringify({ provisioner: 'pd.csi.storage.gke.io' });
    assert.equal(kind, 'volumesnapshots.snapshot.storage.k8s.io');
    return JSON.stringify(seed());
  } };
}

test('preflight checks exact native assignment/adoption permissions without persisted writes', () => {
  const { calls, reviews, command } = fixture();
  const result = preflight({}, command);
  assert.equal(result.passed, true);
  for (const [resource, group] of nativeResources) {
    assert.deepEqual(reviews.filter((review) => review.resource === resource),
      ['get', 'list', 'create', 'patch', 'delete'].map((verb) => ({
        namespace: 'chrono-bot', group, resource, subresource: '', verb,
      })));
  }
  for (const [resource, group, subresource, verb] of [
    ['pods', '', 'exec', 'get'], ['pods', '', 'exec', 'create'],
    ['persistentvolumeclaims', '', '', 'patch'],
    ['sandboxclaims', 'extensions.agents.x-k8s.io', '', 'delete'],
    ['sandboxes', 'agents.x-k8s.io', '', 'get'],
    ['volumesnapshots', 'snapshot.storage.k8s.io', '', 'get'],
  ]) {
    assert.ok(reviews.some((review) => review.resource === resource && review.group === group
      && review.subresource === subresource && review.verb === verb));
  }
  assert.ok(calls.every((args) => !args.includes('secrets')));
  assert.ok(calls.every((args) => ['get', 'create'].includes(args[5])));
  assert.ok(reviews.every((review) => !review.resource.includes('.') && !review.resource.includes('/')
    && !Object.hasOwn(review, 'name')));
});

test('missing APIs or denied rights fail readiness with bounded static diagnostics', () => {
  const { command } = fixture((args, review) => args[6] === 'customresourcedefinition' || review?.subresource === 'exec');
  const result = preflight({}, command);
  assert.equal(result.passed, false);
  assert.equal(result.checks.filter((value) => !value.passed).length, 6);
  assert.ok(!JSON.stringify(result).includes('sensitive raw'));
  for (const options of [
    { context: '--all' }, { namespace: '../other' }, { serviceAccount: 'bad name' },
    { runtimeContext: '--all' }, { runtimeNamespace: '../other' }, { runtimePrincipal: 'bad name' },
    { runtimeContext: productionRuntimeContext },
  ]) {
    assert.throws(() => preflight(options, () => { throw new Error('must not execute'); }));
  }
});

test('raw authorization reviews preserve the explicit remote principal and fail closed on uncertain replies', () => {
  const options = { ...remoteOptions, runtimePrincipal: 'native-controller@example.iam.gserviceaccount.com' };
  for (const reply of [
    () => JSON.stringify({ status: { allowed: false, denied: true, reason: 'sensitive denial detail' } }),
    () => JSON.stringify({ status: { allowed: true, denied: true } }),
    () => JSON.stringify({ status: { allowed: 'true' } }),
    () => JSON.stringify({ status: {} }),
    () => JSON.stringify({}),
    () => 'malformed sensitive reply',
    () => { throw new Error('sensitive impersonation or transport failure'); },
  ]) {
    const { command } = fixture(() => false, remoteConfig, options);
    const result = preflight(options, (args, input) => {
      const raw = command(args, input);
      const review = input && JSON.parse(input).spec.resourceAttributes;
      return review?.resource === 'pods' && review.subresource === 'exec' && review.verb === 'create'
        ? reply() : raw;
    });
    assert.equal(result.passed, false);
    assert.deepEqual(result.checks.filter((check) => !check.passed).map((check) => check.name),
      ['RBAC pods/exec create']);
    assert.ok(!JSON.stringify(result).includes('sensitive'));
  }
});

test('runtime Pod token isolation does not require a disabled ServiceAccount default', () => {
  for (const automount of [undefined, false, true]) {
    const { command } = fixture(() => false, remoteConfig, remoteOptions);
    const result = preflight(remoteOptions, (args, input) => {
      const raw = command(args, input);
      if (args[6] !== 'serviceaccount') return raw;
      return JSON.stringify({ ...JSON.parse(raw), automountServiceAccountToken: automount });
    });
    assert.equal(result.passed, true, `ServiceAccount automount default ${automount}`);
  }
});

test('runtime identity still requires the configured live ServiceAccount in its namespace', () => {
  for (const changed of [
    undefined,
    {},
    { name: 'other-runtime', namespace: remoteOptions.runtimeNamespace },
    { name: 'cma-runtime', namespace: 'other-namespace' },
    { name: 'cma-runtime', namespace: remoteOptions.runtimeNamespace, deletionTimestamp: '2026-10-13T00:00:00Z' },
  ]) {
    const { command } = fixture(() => false, remoteConfig, remoteOptions);
    const result = preflight(remoteOptions, (args, input) => {
      const raw = command(args, input);
      if (args[6] !== 'serviceaccount') return raw;
      if (changed === undefined) throw new Error('ServiceAccount not found');
      return JSON.stringify({ metadata: changed });
    });
    assert.equal(result.passed, false);
    assert.deepEqual(result.checks.filter((check) => !check.passed).map((check) => check.name),
      ['configured runtime ServiceAccount exists and is not deleting']);
  }
});

test('two-cluster preflight reads CMA config locally and native resources and authority remotely', () => {
  const { calls, command } = fixture(() => false, remoteConfig, remoteOptions);
  const result = preflight(remoteOptions, command);
  assert.equal(result.passed, true);
  assert.equal(result.runtimeContext, productionRuntimeContext);
  assert.equal(result.runtimeNamespace, remoteOptions.runtimeNamespace);
  assert.equal(result.runtimePrincipal, remoteOptions.runtimePrincipal);
  assert.equal(calls.filter((args) => args[1] === productionContext).length, 2);
  assert.ok(calls.some((args) => args[6] === 'volumesnapshots.snapshot.storage.k8s.io' && args[7] === 'remote-seed'));
  assert.ok(calls.some((args) => args[6] === 'storageclass' && args[7] === 'remote-disk'));
  assert.ok(calls.every((args) => !args.includes('secrets')));
});

test('selected runtime class resolves Deployment overrides in the runtime cluster', () => {
  const env = [{ name: 'CHRONO_AGENT_SANDBOX_RUNTIME_CLASS', value: 'microvm' }];
  const { command, calls } = fixture(() => false, remoteConfig, remoteOptions, env);
  assert.equal(preflight(remoteOptions, command).passed, true);
  assert.ok(calls.some((args) => args[1] === productionRuntimeContext
    && args[6] === 'runtimeclass.node.k8s.io' && args[7] === 'microvm'));
  const optional = fixture(() => false, remoteConfig, remoteOptions);
  assert.equal(preflight(remoteOptions, optional.command).passed, true);
  assert.ok(optional.calls.every((args) => args[6] !== 'runtimeclass.node.k8s.io'));
});

test('selected runtime class must exist, retain its name and handler, and not be deleting', () => {
  const config = { ...remoteConfig, CHRONO_AGENT_SANDBOX_RUNTIME_CLASS: 'microvm' };
  for (const changed of [
    undefined, {},
    { metadata: { name: 'another-class' }, handler: 'test-handler' },
    { metadata: { name: 'microvm', deletionTimestamp: '2026-10-13T00:00:00Z' }, handler: 'test-handler' },
    { metadata: { name: 'microvm' } },
    { metadata: { name: 'microvm' }, handler: '' },
  ]) {
    const { command } = fixture(() => false, config, remoteOptions);
    const result = preflight(remoteOptions, (args, input) => {
      const raw = command(args, input);
      if (args[6] !== 'runtimeclass.node.k8s.io') return raw;
      if (changed === undefined) throw new Error('sensitive RuntimeClass detail');
      return JSON.stringify(changed);
    });
    assert.equal(result.passed, false);
    assert.deepEqual(result.checks.filter((check) => !check.passed).map((check) => check.name),
      ['configured runtime class exists and matches known production admission']);
    assert.ok(!JSON.stringify(result).includes('sensitive'));
  }
});

test('only known southeast production namespace requires its admitted runtime classes', () => {
  const options = { ...remoteOptions, runtimeNamespace: 'chrono-pod' };
  for (const runtimeClass of [undefined, '', 'unconfined', 'microvm', 'gvisor']) {
    const config = { ...remoteConfig, CHRONO_AGENT_SANDBOX_NAMESPACE: 'chrono-pod', CHRONO_AGENT_SANDBOX_RUNTIME_CLASS: runtimeClass };
    const { command } = fixture(() => false, config, options);
    assert.equal(preflight(options, command).passed, ['microvm', 'gvisor'].includes(runtimeClass));
  }
  for (const [config, elsewhere] of [
    [remoteConfig, remoteOptions],
    [{ ...remoteConfig, CHRONO_AGENT_SANDBOX_NAMESPACE: 'chrono-pod' }, { ...options, runtimeContext: 'another-cluster' }],
    [{ ...remoteConfig, CHRONO_AGENT_SANDBOX_RUNTIME_CLASS: 'custom-runtime' }, remoteOptions],
  ]) {
    assert.equal(preflight(elsewhere, fixture(() => false, config, elsewhere).command).passed, true);
  }
});

test('native storage overrides are complete and legacy storage stays the default', () => {
  assert.equal(homePolicy(legacyConfig).template, 'home-seed');
  assert.equal(homePolicy(remoteConfig).template, 'remote-seed');
  for (const field of ['STORAGE_CLASS', 'CAPACITY_GIB', 'TEMPLATE', 'TEMPLATE_UID']) {
    assert.throws(() => homePolicy({ ...remoteConfig, [`CHRONO_AGENT_SANDBOX_HOME_${field}`]: '' }));
  }
  for (const capacity of ['0', '1025', '1.5', 'not-a-number']) {
    assert.throws(() => homePolicy({ ...remoteConfig, CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB: capacity }));
  }
});

test('remote connection and namespace must match actual deployment configuration', () => {
  for (const change of [
    { CHRONO_AGENT_SANDBOX_NAMESPACE: 'other-cma' },
    { CHRONO_AGENT_SANDBOX_TOKEN_FILE: '' },
    { CHRONO_AGENT_SANDBOX_API_SERVER: 'https://other.invalid' },
    { CHRONO_AGENT_SANDBOX_CA_FILE: 'relative/ca.crt' },
  ]) {
    const { command } = fixture(() => false, { ...remoteConfig, ...change }, remoteOptions);
    assert.equal(preflight(remoteOptions, command).passed, false);
  }
});

test('legacy seed fallback is checked in the actual native cluster and cannot pass on local seed existence', () => {
  const config = { ...remoteConfig };
  for (const field of ['STORAGE_CLASS', 'CAPACITY_GIB', 'TEMPLATE', 'TEMPLATE_UID']) delete config[`CHRONO_AGENT_SANDBOX_HOME_${field}`];
  const { command, calls } = fixture((args) => args[6] === 'volumesnapshots.snapshot.storage.k8s.io' && args[7] === 'home-seed', config, remoteOptions);
  const result = preflight(remoteOptions, command);
  assert.equal(result.passed, false);
  assert.deepEqual(result.checks.filter((check) => !check.passed).map((check) => check.name), ['ready retained-home seed and storage class']);
  assert.ok(calls.some((args) => args[1] === productionRuntimeContext && args[7] === 'home-seed'));
});

test('Deployment ConfigMap overrides resolve metadata auth from the control cluster only', () => {
  const config = { ...remoteConfig, CHRONO_AGENT_SANDBOX_API_SERVER: '', CHRONO_AGENT_SANDBOX_NAMESPACE: '', CHRONO_AGENT_SANDBOX_AUTH_MODE: 'google-metadata', CHRONO_AGENT_SANDBOX_TOKEN_FILE: '' };
  const env = ['API_SERVER', 'NAMESPACE', 'CA_FILE'].map((field) => ({ name: `CHRONO_AGENT_SANDBOX_${field}`, valueFrom: { configMapKeyRef: { name: 'chrono-pod-configmap', key: `CHRONO_POD_${field}`, optional: true } } }));
  const extra = { 'chrono-pod-configmap': { CHRONO_POD_API_SERVER: 'https://runtime.invalid', CHRONO_POD_NAMESPACE: remoteOptions.runtimeNamespace, CHRONO_POD_CA_FILE: '/etc/chrono-pod/ca.crt', CHRONO_POD_WARM_POOL: 'unowned-pool', 'ca.crt': 'not-read-by-preflight' } };
  const { command, calls } = fixture(() => false, config, remoteOptions, env, extra);
  assert.equal(preflight(remoteOptions, command).passed, true);
  assert.equal(calls.filter((args) => args[6] === 'configmap' && args[7] === 'chrono-pod-configmap').length, 1);
  assert.ok(calls.filter((args) => args[6] === 'configmap').every((args) => args[1] === productionContext));
  assert.equal(preflight(remoteOptions, fixture(() => false, config, remoteOptions, env).command).passed, false);
  assert.equal(preflight(remoteOptions, fixture(() => false, { ...config, CHRONO_AGENT_SANDBOX_TOKEN_FILE: '/unexpected/token' }, remoteOptions, env, extra).command).passed, false);
});

test('native Secret environment inputs fail without reading Secret values', () => {
  const env = [{ name: 'CHRONO_AGENT_SANDBOX_API_SERVER', valueFrom: { secretKeyRef: { name: 'untrusted', key: 'server' } } }];
  const { command, calls } = fixture(() => false, remoteConfig, remoteOptions, env);
  assert.equal(preflight(remoteOptions, command).passed, false);
  assert.ok(calls.every((args) => !args.includes('secret') && !args.includes('secrets')));
});

const emptyConfig = {
  ...remoteConfig,
  CHRONO_AGENT_SANDBOX_HOME_MODE: 'native_vct_empty_v1',
  CHRONO_AGENT_SANDBOX_HOME_TEMPLATE: '', CHRONO_AGENT_SANDBOX_HOME_TEMPLATE_UID: '',
};
test('empty native homes need no snapshot API or PVC mutation authority', () => {
  const denied = (args, review) => args.some((arg) => arg.includes('snapshot.storage.k8s.io'))
    || review?.group === 'snapshot.storage.k8s.io'
    || (review?.resource === 'persistentvolumeclaims' && ['create', 'patch', 'delete'].includes(review.verb));
  const { command, calls, reviews } = fixture(denied, emptyConfig, remoteOptions);
  const result = preflight(remoteOptions, command);
  assert.equal(result.passed, true);
  assert.ok(calls.every((args) => !denied(args)));
  assert.ok(reviews.every((review) => !denied([], review)));
  assert.ok(reviews.some((review) => review.resource === 'persistentvolumeclaims' && review.verb === 'get'));
  assert.ok(calls.some((args) => args[6] === 'storageclass' && args[7] === 'remote-disk'));
  for (const [deniedResource, subresource] of [['persistentvolumeclaims', ''], ['pods', ''], ['pods', 'exec']]) {
    const fixtureArgs = fixture((args, review) => review?.resource === deniedResource
      && review.subresource === subresource, emptyConfig, remoteOptions);
    assert.equal(preflight(remoteOptions, fixtureArgs.command).passed, false, deniedResource);
  }
});

test('empty native mode uses explicit storage pair and rejects seed mixing or malformed capacity', () => {
  assert.equal(homePolicy(emptyConfig).mode, 'native_vct_empty_v1');
  assert.equal(homePolicy({ ...emptyConfig, CHRONO_AGENT_SANDBOX_HOME_MODE: '' }).mode, 'native_vct_full_home_v2');
  const borrowed = homePolicy({ ...legacyConfig, CHRONO_AGENT_SANDBOX_HOME_MODE: 'native_vct_empty_v1' });
  assert.equal(borrowed.storageClass, 'cma-home-v1');
  assert.equal(borrowed.template, undefined);
  for (const change of [
    { CHRONO_AGENT_SANDBOX_HOME_TEMPLATE: 'forbidden' },
    { CHRONO_AGENT_SANDBOX_HOME_TEMPLATE_UID: 'forbidden-uid' },
    { CHRONO_AGENT_SANDBOX_HOME_MODE: 'unknown' },
    { CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB: '032' },
  ]) assert.throws(() => homePolicy({ ...emptyConfig, ...change }));
});

test('full home v2 is explicit or inferred and has the same controller storage prerequisites', () => {
  const config = { ...emptyConfig, CHRONO_AGENT_SANDBOX_HOME_MODE: 'native_vct_full_home_v2' };
  const { command } = fixture(() => false, config, remoteOptions);
  assert.equal(preflight(remoteOptions, command).passed, true);
  assert.equal(homePolicy(config).mode, 'native_vct_full_home_v2');
  const borrowed = homePolicy({ ...legacyConfig, CHRONO_AGENT_SANDBOX_HOME_MODE: 'native_vct_full_home_v2' });
  assert.equal(borrowed.storageClass, 'cma-home-v1');
  assert.equal(borrowed.template, undefined);
  for (const field of ['TEMPLATE', 'TEMPLATE_UID']) {
    assert.throws(() => homePolicy({ ...config, [`CHRONO_AGENT_SANDBOX_HOME_${field}`]: 'forbidden' }));
  }
});
