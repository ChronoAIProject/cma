import assert from 'node:assert/strict';
import test from 'node:test';
import { nativeResources, preflight, productionContext, validateCrd, validateSeed } from './agent-sandbox-preflight.mjs';

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

function fixture(fail = () => false) {
  const calls = [];
  return { calls, command(args) {
    calls.push(args);
    assert.deepEqual(args.slice(0, 5), ['--context', productionContext, '--namespace', 'chrono-bot', '--request-timeout=10s']);
    const [operation, kind, name] = args.slice(5);
    if (fail(args)) throw new Error('sensitive raw cluster detail');
    if (operation === 'auth') {
      assert.equal(kind, 'can-i');
      assert.deepEqual(args.slice(-2), ['--as', 'system:serviceaccount:chrono-bot:chrono-bot-ksa']);
      return 'yes\n';
    }
    assert.equal(operation, 'get');
    assert.deepEqual(args.slice(-2), ['-o', 'json']);
    if (kind === 'customresourcedefinition') return JSON.stringify(crd(nativeResources.find(([plural, group]) => name === `${plural}.${group}`)));
    if (kind === 'configmap') return JSON.stringify({ data: {
      CHRONO_SANDBOX_HOME_TEMPLATE: 'home-seed', CHRONO_SANDBOX_HOME_TEMPLATE_UID: 'original-seed',
      CHRONO_SANDBOX_HOME_STORAGE_CLASS: 'cma-home-v1', CHRONO_AGENT_SANDBOX_SERVICE_ACCOUNT: 'cma-runtime',
    } });
    if (kind === 'serviceaccount') return JSON.stringify({ automountServiceAccountToken: false });
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
  for (const options of [{ context: '--all' }, { namespace: '../other' }, { serviceAccount: 'bad name' }]) {
    assert.throws(() => preflight(options, () => { throw new Error('must not execute'); }));
  }
});
