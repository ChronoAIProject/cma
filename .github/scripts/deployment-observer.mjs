import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { lstatSync, readFileSync } from 'node:fs';
import { dirname, isAbsolute, parse, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const source = 'https://github.com/ChronoAIProject/cma';
const sha = /^sha256:[0-9a-f]{64}$/;
const commit = /^[0-9a-f]{40}$/;
const name = /^[a-z0-9](?:[-a-z0-9.]{0,251}[a-z0-9])?$/;
const indexTypes = ['application/vnd.oci.image.index.v1+json', 'application/vnd.docker.distribution.manifest.list.v2+json'];
const manifestTypes = ['application/vnd.oci.image.manifest.v1+json', 'application/vnd.docker.distribution.manifest.v2+json'];
const configTypes = ['application/vnd.oci.image.config.v1+json', 'application/vnd.docker.container.image.v1+json'];
class ObservationError extends Error {}
const check = (condition, code) => { if (!condition) throw new ObservationError(code); };
const record = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
const equal = (left, right) => JSON.stringify(left) === JSON.stringify(right);
const keys = (value, expected) => check(record(value) && equal(Object.keys(value).sort(), [...expected].sort()), 'invalid_schema');
const hash = (bytes) => `sha256:${createHash('sha256').update(bytes).digest('hex')}`;
const text = (value, pattern, code = 'invalid_identifier') => check(typeof value === 'string' && pattern.test(value), code);

export function reference(value) {
  text(value, /^[a-z0-9][a-z0-9.:/_-]*(?::[A-Za-z0-9_.-]+)?@sha256:[0-9a-f]{64}$/, 'immutable_reference_required');
  const [tagged, digest] = value.split('@');
  const slash = tagged.lastIndexOf('/');
  check(slash > 0, 'explicit_registry_required');
  const colon = tagged.lastIndexOf(':');
  const repository = colon > slash ? tagged.slice(0, colon) : tagged;
  text(repository, /^(?:[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?)(?::[0-9]{1,5})?\/[a-z0-9]+(?:[._-][a-z0-9]+)*(?:\/[a-z0-9]+(?:[._-][a-z0-9]+)*)$/, 'invalid_repository');
  return { repository, digest, canonical: `${repository}@${digest}` };
}

export function runningReference(value) {
  check(typeof value === 'string', 'running_image_id_missing');
  return reference(value.replace(/^(?:docker-pullable|containerd|docker):\/\//, ''));
}

function origin(value) {
  check(typeof value === 'string', 'invalid_origin');
  const url = new URL(value);
  check(url.protocol === 'https:' && !url.username && !url.password && url.origin === value, 'invalid_origin');
  return url;
}

export function validateRequest(value) {
  keys(value, ['schema', 'nonce', 'expected']);
  check(value.schema === 'cma-staging-attestation-request-v1', 'invalid_request_schema');
  text(value.nonce, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
  keys(value.expected, ['origin', 'commit', 'nyxidCommit', 'images']);
  origin(value.expected.origin);
  text(value.expected.commit, commit);
  text(value.expected.nyxidCommit, commit);
  keys(value.expected.images, ['api', 'web', 'runtime']);
  for (const image of Object.values(value.expected.images)) check(reference(image).canonical === image, 'canonical_expected_reference_required');
  return value;
}

function absolute(value) {
  check(typeof value === 'string' && isAbsolute(value) && resolve(value) === value && !/[\r\n\0]/.test(value), 'absolute_real_path_required');
}

export function securePath(path, kind, uid = process.getuid()) {
  absolute(path);
  let component = path;
  while (true) {
    check(!lstatSync(component).isSymbolicLink(), 'symlink_refused');
    if (component === parse(component).root) break;
    component = dirname(component);
  }
  const stat = lstatSync(path);
  check(stat.isFile(), 'regular_file_required');
  if (kind === 'private') check(stat.uid === uid && (stat.mode & 0o777) === 0o600, 'private_file_permissions_required');
  else check((stat.uid === uid || stat.uid === 0) && !(stat.mode & 0o022) && Boolean(stat.mode & 0o111), 'trusted_executable_required');
  return stat;
}

export function validateConfig(value) {
  keys(value, ['schema', 'tools', 'kubeconfig', 'registryAuthFile', 'context', 'namespace', 'api', 'web', 'configMap', 'route', 'runtimePlatform', 'nyxid']);
  check(value.schema === 'cma-deployment-observer-config-v1', 'invalid_config_schema');
  keys(value.tools, ['kubectl', 'skopeo']);
  for (const tool of Object.values(value.tools)) {
    keys(tool, ['path', 'sha256']); absolute(tool.path); text(tool.sha256, /^[0-9a-f]{64}$/);
  }
  absolute(value.kubeconfig); absolute(value.registryAuthFile);
  text(value.context, /^[A-Za-z0-9][A-Za-z0-9_.:/@-]{0,511}$/); text(value.namespace, name);
  for (const workload of [value.api, value.web]) {
    keys(workload, ['deployment', 'container', 'service', 'servicePort']);
    for (const field of ['deployment', 'container', 'service']) text(workload[field], name);
    check(Number.isInteger(workload.servicePort) && workload.servicePort > 0 && workload.servicePort <= 65535, 'invalid_service_port');
  }
  keys(value.configMap, ['name', 'runtimeKey', 'originKey']); text(value.configMap.name, name);
  for (const field of ['runtimeKey', 'originKey']) text(value.configMap[field], /^[A-Z][A-Z0-9_]{0,127}$/);
  check(value.configMap.runtimeKey === 'CHRONO_SANDBOX_IMAGE' && value.configMap.originKey === 'CHRONO_SERVER_PUBLIC_URL', 'unsupported_configmap_keys');
  keys(value.route, ['name', 'parent']); text(value.route.name, name);
  keys(value.route.parent, ['name', 'namespace', 'sectionName']);
  for (const field of Object.values(value.route.parent)) text(field, name);
  keys(value.runtimePlatform, ['os', 'architecture']);
  check(value.runtimePlatform.os === 'linux' && value.runtimePlatform.architecture === 'amd64', 'unsupported_runtime_platform');
  check(value.nyxid !== null, 'nyxid_scope_incomplete');
  keys(value.nyxid, ['context', 'namespace', 'deployment', 'container', 'source']);
  text(value.nyxid.context, /^[A-Za-z0-9][A-Za-z0-9_.:/@-]{0,511}$/);
  for (const field of ['namespace', 'deployment', 'container']) text(value.nyxid[field], name);
  const nyxidSource = new URL(value.nyxid.source);
  check(nyxidSource.protocol === 'https:' && !nyxidSource.username && !nyxidSource.password && !nyxidSource.search && !nyxidSource.hash, 'invalid_nyxid_source');
  return value;
}

export function verifyAuthorityFiles(config) {
  for (const tool of Object.values(config.tools)) {
    const stat = securePath(tool.path, 'executable');
    check(stat.size > 0 && stat.size <= 256 * 1024 * 1024, 'invalid_tool_size');
    check(hash(readFileSync(tool.path)) === `sha256:${tool.sha256}`, 'tool_digest_mismatch');
  }
  securePath(config.kubeconfig, 'private');
  securePath(config.registryAuthFile, 'private');
}

const execute = (path, args) => execFileSync(path, args, {
  timeout: 30000, maxBuffer: 4 * 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'],
  env: { PATH: '/usr/local/bin:/usr/bin:/bin' },
});

export const projection = (paths) => paths.length === 1 ? `jsonpath-as-json={.${paths[0]}}` : `jsonpath-as-json={['${paths.join("','")}']}`;
export const sourceKeysProjection = 'go-template={{printf "{\\"name\\":%q,\\"namespace\\":%q,\\"uid\\":%q,\\"resourceVersion\\":%q,\\"keys\\":[" .metadata.name .metadata.namespace .metadata.uid .metadata.resourceVersion}}{{$first := true}}{{range $key, $unused := .data}}{{if not $first}},{{end}}{{printf "%q" $key}}{{$first = false}}{{end}}],"deleting":{{if .metadata.deletionTimestamp}}true{{else}}false{{end}}}';

// kubectl unions cannot nest array expressions: query those separately. Each
// command returns a real JSON array without environment or Secret values.
function reader(config, command) {
  const argumentsFor = (context, namespace, resource, resourceName) => {
    const args = ['--kubeconfig', config.kubeconfig, '--context', context, '--namespace', namespace, '--request-timeout=20s', 'get', resource];
    if (resourceName !== null) { text(resourceName, name); args.push(resourceName); }
    return args;
  };
  const get = (context, namespace, resource, resourceName, paths, selector = null) => {
    if (paths.length > 1 && paths.some((path) => path.includes('['))) return paths.flatMap((path) => get(context, namespace, resource, resourceName, [path], selector));
    const args = argumentsFor(context, namespace, resource, resourceName);
    if (selector !== null) args.push('--selector', selector);
    args.push('-o', projection(paths));
    const output = JSON.parse(command(config.tools.kubectl.path, args).toString('utf8'));
    check(Array.isArray(output), 'invalid_kubernetes_projection');
    return output;
  };
  get.sourceKeys = (context, namespace, resource, resourceName) => {
    check(['configmap', 'secret'].includes(resource), 'invalid_environment_source');
    const args = [...argumentsFor(context, namespace, resource, resourceName), '-o', sourceKeysProjection];
    const value = JSON.parse(command(config.tools.kubectl.path, args).toString('utf8'));
    keys(value, ['name', 'namespace', 'uid', 'resourceVersion', 'keys', 'deleting']);
    check(value.name === resourceName && value.namespace === namespace && value.deleting === false, 'environment_source_identity_incomplete');
    check(typeof value.uid === 'string' && value.uid.length > 0 && typeof value.resourceVersion === 'string' && value.resourceVersion.length > 0, 'environment_source_identity_incomplete');
    check(Array.isArray(value.keys) && value.keys.length <= 4096 && value.keys.every((key) => typeof key === 'string' && /^[-._A-Za-z0-9]+$/.test(key)) && new Set(value.keys).size === value.keys.length, 'environment_source_keys_incomplete');
    value.keys.sort();
    return value;
  };
  return get;
}

function conditions(value, required, generation = null) {
  check(Array.isArray(value), 'conditions_missing');
  for (const type of required) {
    const matches = value.filter((item) => item.type === type);
    check(matches.length === 1 && matches[0].status === 'True', 'resource_not_ready');
    if (generation !== null) check(matches[0].observedGeneration === generation, 'stale_route_status');
  }
}

function selector(value) {
  check(record(value) && record(value.matchLabels) && Object.keys(value.matchLabels).length > 0 && !(value.matchExpressions?.length), 'unsupported_deployment_selector');
  return Object.entries(value.matchLabels).sort().map(([key, value]) => {
    text(key, /^[A-Za-z0-9][A-Za-z0-9_.\/-]{0,252}$/); text(value, /^[A-Za-z0-9][A-Za-z0-9_.-]{0,62}$/);
    return `${key}=${value}`;
  }).join(',');
}

function containerImages(values) {
  check(values.length > 0 && values.length % 2 === 0, 'container_projection_incomplete');
  const count = values.length / 2;
  check(new Set(values.slice(0, count)).size === count, 'duplicate_container');
  return Object.fromEntries(values.slice(0, count).map((name, index) => [name, values[index + count]]));
}

function ownership(owners, kind) {
  check(Array.isArray(owners), 'ownership_missing');
  const controllers = owners.filter((owner) => owner.controller === true);
  check(controllers.length === 1 && controllers[0].kind === kind && controllers[0].apiVersion === 'apps/v1', 'foreign_resource_owner');
  text(controllers[0].name, name);
  check(typeof controllers[0].uid === 'string' && controllers[0].uid.length > 0, 'owner_uid_missing');
  return controllers[0];
}

function workload(get, scope, expectedReference = null) {
  const { context, namespace, deployment, container } = scope;
  const dep = get(context, namespace, 'deployment', deployment, ['metadata.name', 'metadata.namespace', 'metadata.uid', 'metadata.generation', 'metadata.resourceVersion', 'spec.replicas', 'spec.selector', 'spec.template.metadata.labels', 'status', 'metadata.deletionTimestamp']);
  check(dep.length === 9 && dep[0] === deployment && dep[1] === namespace, 'deployment_identity_or_deletion');
  const [,, uid, generation,, replicas, labels, templateLabels, status] = dep;
  check(Number.isInteger(generation) && generation > 0 && Number.isInteger(replicas) && replicas > 0 && replicas <= 32, 'invalid_deployment_generation_or_scale');
  check(status.observedGeneration === generation, 'deployment_generation_unobserved');
  for (const key of ['replicas', 'updatedReplicas', 'readyReplicas', 'availableReplicas']) check(status[key] === replicas, 'deployment_not_settled');
  check(!status.unavailableReplicas, 'deployment_unavailable'); conditions(status.conditions, ['Available', 'Progressing']);
  const images = containerImages(get(context, namespace, 'deployment', deployment, ['spec.template.spec.containers[*].name', 'spec.template.spec.containers[*].image']));
  check(Object.hasOwn(images, container), 'target_container_missing');
  const desired = reference(images[container]);
  if (expectedReference) check(desired.canonical === expectedReference, 'deployment_image_mismatch');
  const podNames = get(context, namespace, 'pods', null, ['items[*].metadata.name'], selector(labels)).sort();
  check(podNames.length === replicas && new Set(podNames).size === replicas, 'pod_population_not_settled');
  const pods = [];
  for (const podName of podNames) {
    const pod = get(context, namespace, 'pod', podName, ['metadata.name', 'metadata.namespace', 'metadata.uid', 'metadata.resourceVersion', 'metadata.ownerReferences', 'metadata.labels', 'spec.nodeName', 'status', 'metadata.deletionTimestamp']);
    check(pod.length === 8 && pod[0] === podName && pod[1] === namespace, 'pod_identity_or_deletion');
    const owner = ownership(pod[4], 'ReplicaSet');
    const rs = get(context, namespace, 'replicaset', owner.name, ['metadata.name', 'metadata.namespace', 'metadata.uid', 'metadata.ownerReferences', 'metadata.deletionTimestamp']);
    check(rs.length === 4 && rs[0] === owner.name && rs[1] === namespace && rs[2] === owner.uid, 'replicaset_identity_or_deletion');
    const parent = ownership(rs[3], 'Deployment');
    check(parent.name === deployment, 'pod_owner_name_mismatch');
    check(parent.uid === uid, 'pod_owner_uid_mismatch');
    for (const [key, value] of Object.entries(labels.matchLabels)) check(pod[5]?.[key] === value, 'pod_selector_mismatch');
    const podImages = containerImages(get(context, namespace, 'pod', podName, ['spec.containers[*].name', 'spec.containers[*].image']));
    check(equal(podImages, images), 'pod_template_images_mismatch');
    check(pod[7].phase === 'Running', 'pod_not_running'); conditions(pod[7].conditions, ['Ready', 'ContainersReady']);
    const statuses = pod[7].containerStatuses;
    check(Array.isArray(statuses) && statuses.length === Object.keys(images).length && new Set(statuses.map((entry) => entry.name)).size === statuses.length, 'container_status_incomplete');
    for (const entry of statuses) check(Object.hasOwn(images, entry.name) && entry.ready === true && entry.started !== false && record(entry.state?.running), 'container_not_ready');
    const target = statuses.filter((entry) => entry.name === container);
    check(target.length === 1, 'target_container_status_missing');
    const node = get(context, namespace, 'node', pod[6], ['metadata.name', 'status.nodeInfo.operatingSystem', 'status.nodeInfo.architecture']);
    check(node.length === 3 && node[0] === pod[6] && node[1] === 'linux' && ['amd64', 'arm64'].includes(node[2]), 'node_platform_incomplete');
    const running = runningReference(target[0].imageID);
    check(running.repository === desired.repository, 'running_repository_mismatch');
    pods.push({ identity: pod.slice(0, 7), imageID: target[0].imageID, digest: running.digest, platform: { os: node[1], architecture: node[2] } });
  }
  check(pods.every((pod) => pod.digest === pods[0].digest && equal(pod.platform, pods[0].platform)), 'ambiguous_running_platform');
  return { desired, platform: pods[0].platform, runningImageID: pods[0].imageID, platformDigest: pods[0].digest, snapshot: { dep, images, pods }, templateLabels };
}

function bindService(get, config, target, observed) {
  const values = get(config.context, config.namespace, 'service', target.service, ['metadata.name', 'metadata.namespace', 'metadata.uid', 'metadata.resourceVersion', 'spec.selector', 'spec.ports', 'spec.type', 'metadata.deletionTimestamp']);
  check(values.length === 7 && values[0] === target.service && values[1] === config.namespace && values[6] === 'ClusterIP', 'service_identity_incomplete');
  check(record(values[4]) && Object.keys(values[4]).length > 0, 'service_selector_missing');
  for (const [key, value] of Object.entries(values[4])) check(observed.templateLabels[key] === value, 'service_selector_mismatch');
  const selectedNames = get(config.context, config.namespace, 'pods', null, ['items[*].metadata.name'], selector({ matchLabels: values[4] })).sort();
  const expectedPods = observed.snapshot.pods;
  check(equal(selectedNames, expectedPods.map((pod) => pod.identity[0]).sort()), 'service_pod_population_mismatch');
  const selectedPods = selectedNames.map((podName) => {
    const identity = get(config.context, config.namespace, 'pod', podName, ['metadata.name', 'metadata.namespace', 'metadata.uid', 'metadata.resourceVersion', 'metadata.labels', 'metadata.deletionTimestamp']);
    const expected = expectedPods.find((pod) => pod.identity[0] === podName).identity;
    check(identity.length === 5 && equal(identity.slice(0, 4), expected.slice(0, 4)) && equal(identity[4], expected[5]), 'service_pod_identity_mismatch');
    for (const [key, value] of Object.entries(values[4])) check(identity[4]?.[key] === value, 'service_pod_selector_mismatch');
    return identity;
  });
  const ports = values[5].filter((port) => port.port === target.servicePort && (port.protocol ?? 'TCP') === 'TCP');
  check(ports.length === 1, 'service_port_mismatch');
  const containerPorts = get(config.context, config.namespace, 'deployment', target.deployment, [`spec.template.spec.containers[?(@.name=="${target.container}")].ports[*]`]);
  check(containerPorts.some((port) => typeof ports[0].targetPort === 'string' ? port.name === ports[0].targetPort : port.containerPort === (ports[0].targetPort ?? ports[0].port)), 'service_target_port_mismatch');
  return { values, selectedPods };
}

function bindEnvironment(get, config) {
  const containerPath = `spec.template.spec.containers[?(@.name=="${config.api.container}")]`;
  const envFrom = get(config.context, config.namespace, 'deployment', config.api.deployment, [`${containerPath}.envFrom[*]`]);
  check(envFrom.length <= 32, 'environment_sources_exceeded');
  for (const entry of envFrom) {
    const sourceKind = entry.configMapRef ? 'configMapRef' : 'secretRef';
    keys(entry, [...(Object.hasOwn(entry, 'prefix') ? ['prefix'] : []), sourceKind]);
    text(entry.prefix ?? '', /^[A-Za-z_][A-Za-z0-9_]*$|^$/);
    keys(entry[sourceKind], ['name', ...(Object.hasOwn(entry[sourceKind], 'optional') ? ['optional'] : [])]);
    text(entry[sourceKind].name, name);
    if (Object.hasOwn(entry[sourceKind], 'optional')) check(typeof entry[sourceKind].optional === 'boolean', 'invalid_environment_source');
  }
  const matches = envFrom.map((entry, index) => entry.configMapRef?.name === config.configMap.name ? index : -1).filter((index) => index >= 0);
  check(matches.length === 1, 'api_configmap_not_bound');
  const targetIndex = matches[0];
  check((envFrom[targetIndex].prefix ?? '') === '', 'api_configmap_prefixed');
  const protectedKeys = [config.configMap.runtimeKey, config.configMap.originKey];
  const laterSources = envFrom.slice(targetIndex + 1).map((entry) => {
    const resource = entry.configMapRef ? 'configmap' : 'secret';
    const ref = entry.configMapRef ?? entry.secretRef;
    const identity = get.sourceKeys(config.context, config.namespace, resource, ref.name);
    check(!identity.keys.some((key) => protectedKeys.includes(`${entry.prefix ?? ''}${key}`)), 'api_configmap_shadowed');
    return { resource, identity };
  });
  const envNames = get(config.context, config.namespace, 'deployment', config.api.deployment, [`${containerPath}.env[*].name`]);
  check(!envNames.some((key) => protectedKeys.includes(key)), 'api_configmap_explicitly_overridden');
  return { envFrom, laterSources, envNames };
}

function bindRoute(get, config, expectedOrigin) {
  const value = get(config.context, config.namespace, 'httproute', config.route.name, ['metadata.name', 'metadata.namespace', 'metadata.uid', 'metadata.generation', 'metadata.resourceVersion', 'spec.hostnames', 'spec.parentRefs', 'spec.rules', 'status.parents', 'metadata.deletionTimestamp']);
  check(value.length === 9 && value[0] === config.route.name && value[1] === config.namespace, 'route_identity_incomplete');
  const url = origin(expectedOrigin);
  check(!url.port && equal(value[5], [url.hostname]), 'route_origin_mismatch');
  const matchesParent = (parent) => parent.name === config.route.parent.name && (parent.namespace ?? config.namespace) === config.route.parent.namespace && parent.sectionName === config.route.parent.sectionName && (parent.group ?? 'gateway.networking.k8s.io') === 'gateway.networking.k8s.io' && (parent.kind ?? 'Gateway') === 'Gateway';
  check(value[6].length === 1 && matchesParent(value[6][0]), 'route_parent_mismatch');
  const statuses = value[8].filter((entry) => matchesParent(entry.parentRef));
  check(statuses.length === 1, 'route_parent_status_missing'); conditions(statuses[0].conditions, ['Accepted', 'ResolvedRefs'], value[3]);
  const observed = [];
  for (const rule of value[7]) {
    check(!rule.filters?.length && rule.backendRefs?.length === 1 && Array.isArray(rule.matches), 'route_filter_or_backend_ambiguity');
    const backend = rule.backendRefs[0];
    check((backend.group ?? '') === '' && (backend.kind ?? 'Service') === 'Service' && (backend.namespace ?? config.namespace) === config.namespace && (backend.weight ?? 1) === 1 && !backend.filters?.length, 'route_backend_mismatch');
    for (const match of rule.matches) {
      keys(match, ['path']); keys(match.path, ['type', 'value']);
      observed.push([match.path.type, match.path.value, backend.name, backend.port]);
    }
  }
  const expected = [['PathPrefix', '/api', config.api.service, config.api.servicePort], ['PathPrefix', '/auth', config.api.service, config.api.servicePort], ['Exact', '/openapi.json', config.api.service, config.api.servicePort], ['Exact', '/.well-known/agent-card.json', config.api.service, config.api.servicePort], ['PathPrefix', '/', config.web.service, config.web.servicePort]];
  check(equal(observed.sort(), expected.sort()), 'route_path_mapping_mismatch');
  return value;
}

export function registryImage(config, command, imageReference, platform, expectedSource, expectedCommit, runtime = false, runningDigest = null) {
  const ref = reference(imageReference);
  const inspect = (digest, configuration = false) => command(config.tools.skopeo.path, ['--command-timeout=25s', 'inspect', '--authfile', config.registryAuthFile, '--tls-verify=true', '--raw', ...(configuration ? ['--config'] : []), `docker://${ref.repository}@${digest}`]);
  const decode = (bytes, digest, size = null) => {
    check(Buffer.isBuffer(bytes) && hash(bytes) === digest && (size === null || bytes.length === size), 'registry_content_digest_mismatch');
    const value = JSON.parse(bytes.toString('utf8')); check(record(value), 'invalid_registry_document'); return value;
  };
  const descriptor = (value) => { check(record(value) && sha.test(value.digest) && Number.isInteger(value.size) && value.size > 0, 'invalid_registry_descriptor'); };
  const root = decode(inspect(ref.digest), ref.digest);
  check(root.schemaVersion === 2, 'unsupported_manifest_schema');
  let manifest = root;
  let platformDigest = ref.digest;
  if (indexTypes.includes(root.mediaType)) {
    check(Array.isArray(root.manifests) && root.manifests.length > 0 && root.manifests.length <= 64, 'invalid_registry_index');
    const candidates = root.manifests.filter((item) => item.platform?.os === platform.os && item.platform?.architecture === platform.architecture && (!item.platform.variant || (platform.architecture === 'arm64' && item.platform.variant === 'v8')));
    check(candidates.length === 1, 'ambiguous_registry_platform'); descriptor(candidates[0]);
    check(manifestTypes.includes(candidates[0].mediaType), 'nested_registry_index_refused');
    platformDigest = candidates[0].digest;
    manifest = decode(inspect(platformDigest), platformDigest, candidates[0].size);
  }
  check(manifest.schemaVersion === 2 && manifestTypes.includes(manifest.mediaType), 'unsupported_platform_manifest');
  if (runningDigest !== null) check(runningDigest === platformDigest, 'running_image_is_not_platform_manifest');
  descriptor(manifest.config); check(configTypes.includes(manifest.config.mediaType), 'unsupported_image_config');
  const image = decode(inspect(platformDigest, true), manifest.config.digest, manifest.config.size);
  check(image.os === platform.os && image.architecture === platform.architecture && (!image.variant || (image.architecture === 'arm64' && image.variant === 'v8')), 'config_platform_mismatch');
  check(image.config?.Labels?.['org.opencontainers.image.source'] === expectedSource && image.config?.Labels?.['org.opencontainers.image.revision'] === expectedCommit, 'registry_source_or_revision_mismatch');
  if (runtime) check(image.config.Labels['dev.chrono.cma.runtime-contract'] === '3', 'runtime_contract_mismatch');
  return { reference: ref.canonical, rootDigest: ref.digest, platformDigest, configDigest: manifest.config.digest, source: image.config.Labels['org.opencontainers.image.source'], revision: image.config.Labels['org.opencontainers.image.revision'] };
}

export function observe(config, request, command = execute, clock = () => new Date()) {
  validateConfig(config); validateRequest(request);
  const started = clock().getTime();
  const get = reader(config, (path, args) => { check(clock().getTime() - started <= 240000, 'observation_deadline'); return command(path, args); });
  const snapshot = () => {
    const api = workload(get, { ...config.api, context: config.context, namespace: config.namespace }, request.expected.images.api);
    const web = workload(get, { ...config.web, context: config.context, namespace: config.namespace }, request.expected.images.web);
    const nyxid = workload(get, config.nyxid);
    const map = get(config.context, config.namespace, 'configmap', config.configMap.name, ['metadata.name', 'metadata.namespace', 'metadata.uid', 'metadata.resourceVersion', `data.${config.configMap.runtimeKey}`, `data.${config.configMap.originKey}`, 'metadata.deletionTimestamp']);
    check(map.length === 6 && map[0] === config.configMap.name && map[1] === config.namespace, 'configmap_identity_incomplete');
    check(reference(map[4]).canonical === request.expected.images.runtime && map[5] === request.expected.origin, 'configured_runtime_or_origin_mismatch');
    const environment = bindEnvironment(get, config);
    return { api, web, nyxid, map, environment, services: [bindService(get, config, config.api, api), bindService(get, config, config.web, web)], route: bindRoute(get, config, request.expected.origin) };
  };
  const before = snapshot();
  const images = {};
  for (const component of ['api', 'web']) {
    const observed = before[component];
    images[component] = { ...registryImage(config, command, observed.desired.canonical, observed.platform, source, request.expected.commit, false, observed.platformDigest), runningImageID: observed.runningImageID };
  }
  images.runtime = registryImage(config, command, before.map[4], config.runtimePlatform, source, request.expected.commit, true);
  const nyxid = registryImage(config, command, before.nyxid.desired.canonical, before.nyxid.platform, config.nyxid.source, request.expected.nyxidCommit, false, before.nyxid.platformDigest);
  check(equal(before, snapshot()), 'deployment_changed_during_observation');
  const observedAt = clock();
  check(observedAt.getTime() >= started && observedAt.getTime() - started <= 240000, 'observation_deadline');
  return { schema: 'cma-staging-attestation-v1', nonce: request.nonce, observedAt: observedAt.toISOString(), origin: before.map[5], commit: images.api.revision, nyxidCommit: nyxid.revision, runtimePin: images.runtime.reference, images };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    check(process.argv.length === 2, 'unexpected_arguments');
    const configPath = process.env.CMA_ATTESTATION_CONFIG;
    check(securePath(configPath, 'private').size <= 32768, 'config_too_large');
    const configBytes = readFileSync(configPath);
    const config = validateConfig(JSON.parse(configBytes));
    const chunks = []; let length = 0;
    for await (const chunk of process.stdin) { length += chunk.length; check(length <= 16384, 'request_too_large'); chunks.push(chunk); }
    const request = validateRequest(JSON.parse(Buffer.concat(chunks)));
    verifyAuthorityFiles(config);
    const receipt = observe(config, request);
    verifyAuthorityFiles(config);
    securePath(configPath, 'private'); check(readFileSync(configPath).equals(configBytes), 'config_changed_during_observation');
    process.stdout.write(`${JSON.stringify(receipt)}\n`);
  } catch (error) {
    const code = error instanceof ObservationError ? error.message : 'command_or_input_incomplete';
    process.stderr.write(`Deployment observation incomplete (${code}); raw diagnostics withheld.\n`);
    process.exitCode = 1;
  }
}
