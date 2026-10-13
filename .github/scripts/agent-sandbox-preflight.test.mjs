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
  return { calls, command(args) {
    calls.push(args);
    if (fail(args)) throw new Error('sensitive raw cluster detail');
    if (args[2] === 'config') {
      assert.deepEqual(args, ['--context', options.runtimeContext, 'config', 'view', '--minify', '-o', 'jsonpath={.clusters[0].cluster.server}']);
      return 'https://runtime.invalid';
    }
    const [operation, kind, name] = args.slice(5);
    const control = operation === 'get' && ['configmap', 'deployment'].includes(kind);
    const context = control ? productionContext : options.runtimeContext ?? productionContext;
    const namespace = control ? 'chrono-bot' : options.runtimeNamespace ?? 'chrono-bot';
    assert.deepEqual(args.slice(0, 5), ['--context', context, '--namespace', namespace, '--request-timeout=10s']);
    if (operation === 'auth') {
      assert.equal(kind, 'can-i');
      assert.deepEqual(args.slice(-2), ['--as', options.runtimePrincipal ?? 'system:serviceaccount:chrono-bot:chrono-bot-ksa']);
      return 'yes\n';
    }
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
    if (kind === 'storageclass') return JSON.stringify({ provisioner: 'pd.csi.storage.gke.io' });
    assert.equal(kind, 'volumesnapshots.snapshot.storage.k8s.io');
    return JSON.stringify(seed());
  } };
}

test('preflight only reads a pinned context and checks exact native assignment/adoption permissions', () => {
  const { calls, command } = fixture();
  const result = preflight({}, command);
  assert.equal(result.passed, true);
  for (const [resource, verb] of [['pods/exec', 'get'], ['persistentvolumeclaims', 'patch'], ['sandboxclaims.extensions.agents.x-k8s.io', 'delete']]) {
    assert.ok(calls.some((args) => args[5] === 'auth' && args[7] === verb && args[8] === resource));
  }
  assert.ok(calls.every((args) => !args.includes('secrets')));
});

test('missing APIs or denied rights fail readiness with bounded static diagnostics', () => {
  const { command } = fixture((args) => args[6] === 'customresourcedefinition' || args[8] === 'pods/exec');
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

test('runtime Pod token isolation does not require a disabled ServiceAccount default', () => {
  for (const automount of [undefined, false, true]) {
    const { command } = fixture(() => false, remoteConfig, remoteOptions);
    const result = preflight(remoteOptions, (args) => {
      const raw = command(args);
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
    const result = preflight(remoteOptions, (args) => {
      const raw = command(args);
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
  const env = ['API_SERVER', 'NAMESPACE', 'CA_FILE'].map((field) => ({ name: `CHRONO_AGENT_SANDBOX_${field}`, valueFrom: { configMapKeyRef: { name: 'example-namespace-02', key: `CHRONO_POD_${field}`, optional: true } } }));
  const extra = { 'example-namespace-02': { CHRONO_POD_API_SERVER: 'https://runtime.invalid', CHRONO_POD_NAMESPACE: remoteOptions.runtimeNamespace, CHRONO_POD_CA_FILE: '/etc/example-registry-01/ca.crt', CHRONO_POD_WARM_POOL: 'unowned-pool', 'ca.crt': 'not-read-by-preflight' } };
  const { command, calls } = fixture(() => false, config, remoteOptions, env, extra);
  assert.equal(preflight(remoteOptions, command).passed, true);
  assert.equal(calls.filter((args) => args[6] === 'configmap' && args[7] === 'example-namespace-02').length, 1);
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
  const denied = (args) => args.some((arg) => arg.includes('snapshot.storage.k8s.io'))
    || (args[8] === 'persistentvolumeclaims' && ['create', 'patch', 'delete'].includes(args[7]));
  const { command, calls } = fixture(denied, emptyConfig, remoteOptions);
  const result = preflight(remoteOptions, command);
  assert.equal(result.passed, true);
  assert.ok(calls.every((args) => !denied(args)));
  assert.ok(calls.some((args) => args[8] === 'persistentvolumeclaims' && args[7] === 'get'));
  assert.ok(calls.some((args) => args[6] === 'storageclass' && args[7] === 'remote-disk'));
  for (const deniedResource of ['persistentvolumeclaims', 'pods', 'pods/exec']) {
    const fixtureArgs = fixture((args) => args[8] === deniedResource, emptyConfig, remoteOptions);
    assert.equal(preflight(remoteOptions, fixtureArgs.command).passed, false, deniedResource);
  }
});

test('empty native mode uses explicit storage pair and rejects seed mixing or malformed capacity', () => {
  assert.equal(homePolicy(emptyConfig).mode, 'native_vct_empty_v1');
  assert.equal(homePolicy({ ...emptyConfig, CHRONO_AGENT_SANDBOX_HOME_MODE: '' }).mode, 'native_vct_empty_v1');
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
