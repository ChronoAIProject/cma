import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

export const upstreamRevision = '86ad94178a85aa26788077a55894cdc234c5ea6a';
export const productionContext = 'gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke';
export const productionRuntimeContext = 'gke_chronoai-501608_asia-southeast1-b_chronoai-prod-asia-southeast1-gke';
export const nativeResources = [
  ['sandboxes', 'agents.x-k8s.io', ['podTemplate', 'volumeClaimTemplates', 'operatingMode']],
  ['sandboxtemplates', 'extensions.agents.x-k8s.io', ['podTemplate', 'volumeClaimTemplates', 'volumeClaimTemplatesPolicy']],
  ['sandboxwarmpools', 'extensions.agents.x-k8s.io', ['replicas', 'sandboxTemplateRef', 'updateStrategy']],
  ['sandboxclaims', 'extensions.agents.x-k8s.io', ['warmPoolRef']],
];
const authority = [
  ...nativeResources.map(([plural, group]) => [`${plural}.${group}`, ['get', 'list', 'create', 'patch', 'delete']]),
  ['persistentvolumeclaims', ['get', 'list']],
  ['pods', ['get', 'list']],
  ['pods/exec', ['get', 'create']],
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
const contextName = (value) => typeof value === 'string' && value.length > 0 && !value.startsWith('-') && !/\s/.test(value);
const principalName = (value) => typeof value === 'string' && /^[a-zA-Z0-9][a-zA-Z0-9:._@-]{0,252}$/.test(value);

export function homePolicy(config) {
  const fields = ['STORAGE_CLASS', 'CAPACITY_GIB', 'TEMPLATE', 'TEMPLATE_UID'];
  const native = fields.map((field) => config[`CHRONO_AGENT_SANDBOX_HOME_${field}`]);
  const legacy = fields.map((field) => config[`CHRONO_SANDBOX_HOME_${field}`]);
  const explicit = config.CHRONO_AGENT_SANDBOX_HOME_MODE || undefined;
  assert.ok(explicit === undefined || ['native_vct_empty_v1', 'legacy_snapshot'].includes(explicit));
  const mode = explicit ?? (native.some(Boolean) && !native[2] && !native[3] ? 'native_vct_empty_v1' : 'legacy_snapshot');
  if (mode === 'native_vct_empty_v1') assert.ok(!native[2] && !native[3]);
  const values = mode === 'native_vct_empty_v1'
    ? native.slice(0, 2).map((value, index) => value || (explicit ? legacy[index] : undefined))
    : native.some(Boolean) ? native : legacy;
  assert.ok(values.every((value) => typeof value === 'string' && value.length > 0));
  const [storageClass, capacity, template, templateUid] = values;
  assert.ok(dns(storageClass));
  assert.ok(/^[1-9]\d*$/.test(capacity) && Number(capacity) <= 1024);
  if (mode === 'legacy_snapshot') assert.ok(dns(template) && dns(templateUid));
  return { mode, storageClass, capacity, template, templateUid };
}

export function preflight(options = {}, command = (args) => execFileSync('kubectl', args, {
  encoding: 'utf8', timeout: 15000, maxBuffer: 16 * 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'],
})) {
  const context = options.context ?? productionContext;
  const namespace = options.namespace ?? 'chrono-bot';
  const serviceAccount = options.serviceAccount ?? 'chrono-bot-ksa';
  const runtimeContext = options.runtimeContext ?? context;
  assert.ok(contextName(context) && contextName(runtimeContext));
  assert.ok(dns(namespace) && dns(serviceAccount));
  if (options.runtimeNamespace !== undefined) assert.ok(dns(options.runtimeNamespace));
  if (options.runtimePrincipal !== undefined) assert.ok(principalName(options.runtimePrincipal));
  assert.ok(runtimeContext === context || options.runtimePrincipal, 'cross-cluster checks require the actual remote principal');
  const base = (target, scope) => ['--context', target, '--namespace', scope, '--request-timeout=10s'];
  const checks = [];
  const check = (name, operation) => {
    try { operation(); checks.push({ name, passed: true }); }
    catch { checks.push({ name, passed: false }); }
  };
  let config = {};
  const configMaps = new Map();
  const readConfigMap = (name, optional = false) => {
    assert.ok(dns(name));
    if (configMaps.has(name)) return configMaps.get(name);
    const raw = command([...base(context, namespace), 'get', 'configmap', name, ...(optional ? ['--ignore-not-found'] : []), '-o', 'json']);
    const data = raw.trim() ? JSON.parse(raw).data : undefined;
    assert.ok((optional && data === undefined) || (data && typeof data === 'object' && !Array.isArray(data)));
    configMaps.set(name, data);
    return data;
  };
  check('CMA deployment ConfigMap is readable', () => {
    config = { ...readConfigMap('cma-agent-manager') };
  });
  check('native environment matches the CMA Deployment overrides', () => {
    const deployment = JSON.parse(command([...base(context, namespace), 'get', 'deployment', 'cma-agent-manager', '-o', 'json']));
    const containers = deployment.spec?.template?.spec?.containers;
    assert.ok(Array.isArray(containers) && containers.length === 1 && containers[0].name === 'api');
    for (const env of containers[0].env ?? []) {
      if (!env.name.startsWith('CHRONO_AGENT_SANDBOX_')) continue;
      if (Object.hasOwn(env, 'value')) { config[env.name] = env.value; continue; }
      const source = env.valueFrom?.configMapKeyRef;
      assert.ok(source, 'native settings must use literal or ConfigMap inputs');
      const data = readConfigMap(source.name, source.optional === true);
      if (data?.[source.key] !== undefined) config[env.name] = data[source.key];
      else assert.equal(source.optional, true);
    }
  });
  const configuredNamespace = config.CHRONO_AGENT_SANDBOX_NAMESPACE || config.CHRONO_SANDBOX_NAMESPACE || namespace;
  const runtimeNamespace = options.runtimeNamespace ?? configuredNamespace;
  assert.ok(dns(runtimeNamespace));
  const runtimePrincipal = options.runtimePrincipal ?? `system:serviceaccount:${namespace}:${serviceAccount}`;
  const runtimeBase = base(runtimeContext, runtimeNamespace);
  const get = (kind, name) => JSON.parse(command([...runtimeBase, 'get', kind, name, '-o', 'json']));
  check('native namespace matches CMA configuration', () => assert.equal(runtimeNamespace, configuredNamespace));
  check('native Kubernetes connection matches the selected runtime cluster', () => {
    const mode = config.CHRONO_AGENT_SANDBOX_AUTH_MODE || 'token-file';
    assert.ok(['token-file', 'google-metadata'].includes(mode));
    const remote = ['API_SERVER', 'CA_FILE', 'TOKEN_FILE'].map((field) => config[`CHRONO_AGENT_SANDBOX_${field}`]);
    if (runtimeContext === context && mode === 'token-file' && remote.every((value) => !value)) return;
    const [server, caFile, tokenFile] = remote;
    const required = mode === 'google-metadata' ? [server, caFile] : remote;
    assert.ok(required.every((value) => typeof value === 'string' && value.length > 0));
    if (mode === 'google-metadata') assert.ok(!tokenFile);
    const url = new URL(server);
    assert.ok(url.protocol === 'https:' && !url.username && !url.password && server === url.origin);
    const paths = mode === 'google-metadata' ? [caFile] : [caFile, tokenFile];
    assert.ok(paths.every((path) => path.startsWith('/') && path.length <= 4096 && !/[\x00-\x1f\x7f]/.test(path)));
    const selectedServer = command(['--context', runtimeContext, 'config', 'view', '--minify', '-o', 'jsonpath={.clusters[0].cluster.server}']).trim();
    assert.equal(new URL(selectedServer).origin, server);
  });
  for (const resource of nativeResources) {
    const name = `${resource[0]}.${resource[1]}`;
    check(`API ${name}/v1beta1`, () => validateCrd(get('customresourcedefinition', name), resource));
  }
  let policy;
  check('native home policy is valid', () => { policy = homePolicy(config); });
  const requiredAuthority = [...authority];
  if (policy?.mode === 'legacy_snapshot') {
    requiredAuthority.push(['persistentvolumeclaims', ['create', 'patch', 'delete']],
      ['volumesnapshots.snapshot.storage.k8s.io', ['get']]);
  }
  for (const [resource, verbs] of requiredAuthority) {
    for (const verb of verbs) {
      check(`RBAC ${resource} ${verb}`, () => {
        assert.equal(command([...runtimeBase, 'auth', 'can-i', verb, resource,
          '--as', runtimePrincipal]).trim(), 'yes');
      });
    }
  }
  check('configured runtime ServiceAccount exists and is not deleting', () => {
    const name = config.CHRONO_AGENT_SANDBOX_SERVICE_ACCOUNT || 'cma-runtime';
    assert.ok(dns(name));
    const account = get('serviceaccount', name);
    assert.equal(account.metadata?.name, name);
    assert.equal(account.metadata?.namespace, runtimeNamespace);
    assert.ok(!account.metadata?.deletionTimestamp);
    // Both CMA Pod templates explicitly disable automount. That Pod setting
    // overrides the ServiceAccount default, which need not itself be false.
  });
  check(policy?.mode === 'native_vct_empty_v1' ? 'native home storage class' : 'ready retained-home seed and storage class', () => {
    assert.ok(policy);
    if (policy.mode === 'legacy_snapshot') {
      validateSeed(get('volumesnapshots.snapshot.storage.k8s.io', policy.template), policy.templateUid);
    }
    assert.equal(get('storageclass', policy.storageClass).provisioner, 'pd.csi.storage.gke.io');
  });
  return {
    context, namespace, serviceAccount, runtimeContext, runtimeNamespace, runtimePrincipal, upstreamRevision,
    scope: 'read-only CMA configuration and runtime API schema, RBAC and home prerequisites; mounted credential identity, controller version and live behavior require separate acceptance',
    passed: checks.every((value) => value.passed), checks,
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const args = process.argv.slice(2);
    const options = {};
    const flags = {
      '--context': 'context', '--namespace': 'namespace', '--service-account': 'serviceAccount',
      '--runtime-context': 'runtimeContext', '--runtime-namespace': 'runtimeNamespace', '--runtime-principal': 'runtimePrincipal',
    };
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
    process.stderr.write('Preflight arguments invalid; use --context, --namespace, --service-account, --runtime-context, --runtime-namespace and --runtime-principal. Cross-cluster checks require the remote principal. Raw cluster data withheld.\n');
    process.exitCode = 1;
  }
}
