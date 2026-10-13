import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

export const upstreamRevision = '86ad94178a85aa26788077a55894cdc234c5ea6a';
export const productionContext = 'gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke';
export const nativeResources = [
  ['sandboxes', 'agents.x-k8s.io', ['podTemplate', 'volumeClaimTemplates', 'operatingMode']],
  ['sandboxtemplates', 'extensions.agents.x-k8s.io', ['podTemplate', 'volumeClaimTemplates', 'volumeClaimTemplatesPolicy']],
  ['sandboxwarmpools', 'extensions.agents.x-k8s.io', ['replicas', 'sandboxTemplateRef', 'updateStrategy']],
  ['sandboxclaims', 'extensions.agents.x-k8s.io', ['warmPoolRef']],
];
const authority = [
  ...nativeResources.map(([plural, group]) => [`${plural}.${group}`, ['get', 'list', 'create', 'patch', 'delete']]),
  ['persistentvolumeclaims', ['get', 'list', 'create', 'patch', 'delete']],
  ['pods', ['get', 'list']],
  ['pods/exec', ['get', 'create']],
  ['volumesnapshots.snapshot.storage.k8s.io', ['get']],
];

export function validateCrd(crd, [plural, group, required]) {
  assert.equal(crd.metadata.name, `${plural}.${group}`);
  assert.equal(crd.spec.group, group);
  assert.equal(crd.spec.scope, 'Namespaced');
  assert.equal(crd.spec.names.plural, plural);
  assert.ok(crd.status?.conditions?.some((condition) => condition.type === 'Established' && condition.status === 'True'));
  const version = crd.spec.versions.find((value) => value.name === 'v1beta1' && value.served);
  assert.ok(version, 'served v1beta1 API is required');
  const properties = version.schema?.openAPIV3Schema?.properties;
  const spec = properties?.spec?.properties;
  assert.ok(spec, 'structural spec schema is required');
  for (const field of required) assert.ok(Object.hasOwn(spec, field), `missing spec.${field}`);
  if (plural === 'sandboxes') {
    assert.ok(spec.operatingMode.enum.includes('Running') && spec.operatingMode.enum.includes('Suspended'));
  }
  if (plural === 'sandboxtemplates') assert.ok(spec.volumeClaimTemplatesPolicy.enum.includes('Disallowed'));
  if (plural === 'sandboxwarmpools') {
    assert.ok(spec.updateStrategy.properties.type.enum.includes('OnReplenish'));
    assert.ok(properties.status?.properties?.observedGeneration);
    assert.ok(properties.status?.properties?.readyReplicas);
  }
  if (plural === 'sandboxclaims') assert.ok(properties.status?.properties?.sandbox?.properties?.name);
}

export function validateSeed(seed, expectedUid) {
  assert.equal(seed.metadata.uid, expectedUid);
  assert.equal(seed.metadata.deletionTimestamp, undefined);
  assert.equal(seed.status?.readyToUse, true);
  assert.ok(seed.status?.boundVolumeSnapshotContentName);
  assert.equal(seed.metadata.annotations?.['cma/home-seed-format'], '1');
}

const dns = (value) => typeof value === 'string' && /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(value);

export function preflight(options = {}, command = (args) => execFileSync('kubectl', args, {
  encoding: 'utf8', timeout: 15000, maxBuffer: 16 * 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'],
})) {
  const context = options.context ?? productionContext;
  const namespace = options.namespace ?? 'chrono-bot';
  const serviceAccount = options.serviceAccount ?? 'chrono-bot-ksa';
  assert.ok(typeof context === 'string' && context.length > 0 && !context.startsWith('-') && !/\s/.test(context));
  assert.ok(dns(namespace) && dns(serviceAccount));
  const base = ['--context', context, '--namespace', namespace, '--request-timeout=10s'];
  const checks = [];
  const check = (name, operation) => {
    try { operation(); checks.push({ name, passed: true }); }
    catch { checks.push({ name, passed: false }); }
  };
  const get = (kind, name) => JSON.parse(command([...base, 'get', kind, name, '-o', 'json']));
  for (const resource of nativeResources) {
    const name = `${resource[0]}.${resource[1]}`;
    check(`API ${name}/v1beta1`, () => validateCrd(get('customresourcedefinition', name), resource));
  }
  for (const [resource, verbs] of authority) {
    for (const verb of verbs) {
      check(`RBAC ${resource} ${verb}`, () => {
        assert.equal(command([...base, 'auth', 'can-i', verb, resource,
          '--as', `system:serviceaccount:${namespace}:${serviceAccount}`]).trim(), 'yes');
      });
    }
  }
  check('runtime ServiceAccount has no automatic API token', () => {
    const config = get('configmap', 'cma-api').data;
    const name = config.CHRONO_AGENT_SANDBOX_SERVICE_ACCOUNT || 'cma-runtime';
    assert.ok(dns(name));
    assert.equal(get('serviceaccount', name).automountServiceAccountToken, false);
  });
  check('ready retained-home seed and storage class', () => {
    const config = get('configmap', 'cma-api').data;
    assert.ok(dns(config.CHRONO_SANDBOX_HOME_TEMPLATE));
    assert.ok(dns(config.CHRONO_SANDBOX_HOME_STORAGE_CLASS));
    assert.ok(config.CHRONO_SANDBOX_HOME_TEMPLATE_UID);
    validateSeed(get('volumesnapshots.snapshot.storage.k8s.io', config.CHRONO_SANDBOX_HOME_TEMPLATE), config.CHRONO_SANDBOX_HOME_TEMPLATE_UID);
    assert.equal(get('storageclass', config.CHRONO_SANDBOX_HOME_STORAGE_CLASS).provisioner, 'pd.csi.storage.gke.io');
  });
  return {
    context, namespace, serviceAccount, upstreamRevision,
    scope: 'read-only API schema, RBAC and home prerequisites; controller version and live behavior require separate acceptance',
    passed: checks.every((value) => value.passed), checks,
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const args = process.argv.slice(2);
    const options = {};
    const flags = { '--context': 'context', '--namespace': 'namespace', '--service-account': 'serviceAccount' };
    assert.equal(args.length % 2, 0);
    for (let index = 0; index < args.length; index += 2) {
      const name = flags[args[index]];
      assert.ok(name && !Object.hasOwn(options, name));
      options[name] = args[index + 1];
    }
    const result = preflight(options);
    process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
    if (!result.passed) process.exitCode = 1;
  } catch {
    process.stderr.write('Preflight arguments invalid; use --context, --namespace and --service-account. Raw cluster data withheld.\n');
    process.exitCode = 1;
  }
}
