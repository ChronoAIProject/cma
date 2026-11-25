"""Opt-in native Agent manifests; rendering never creates or starts a resource."""
from copy import deepcopy
import json

from migration_kind_manifests import NAMESPACE
from migration_native_dependencies import (
    IMAGE, dependencies, object_key, owned_cluster, runtime_identity, unique_objects,
)
from migration_native_endpoints import render as render_endpoints

NATIVE_GROUPS = {'agents.x-k8s.io', 'extensions.agents.x-k8s.io'}
NETWORK_POLICY = {
    'ingress': [{'from': [{'podSelector': {}}]}],
    'egress': [{'to': [{'podSelector': {}}]}, {'to': [{
        'namespaceSelector': {'matchLabels': {'kubernetes.io/metadata.name': 'kube-system'}},
        'podSelector': {'matchLabels': {'k8s-app': 'kube-dns'}},
    }], 'ports': [{'protocol': 'UDP', 'port': 53}, {'protocol': 'TCP', 'port': 53}]}],
}
ROLE_NAME = 'cma-runtime-sandbox-reclaimer'
AM_ACCOUNT = 'cma-migration-am'
RUNTIME_ACCOUNT = 'cma-runtime'
APP_RULES = [
    {'apiGroups': [''], 'resources': ['pods', 'persistentvolumeclaims'],
     'verbs': ['get', 'list', 'delete', 'patch']},
    {'apiGroups': ['agents.x-k8s.io'], 'resources': ['sandboxes'],
     'verbs': ['get', 'list', 'create', 'delete', 'patch']},
    {'apiGroups': ['extensions.agents.x-k8s.io'],
     'resources': ['sandboxwarmpools', 'sandboxclaims', 'sandboxtemplates'],
     'verbs': ['get', 'list', 'create', 'delete', 'patch']},
    {'apiGroups': ['sandbox.opensandbox.io'], 'resources': ['batchsandboxes'],
     'verbs': ['get', 'list']},
]


def reject_secret_authority(value):
    if isinstance(value, dict):
        if value.get('kind') == 'Secret' or any(key in value for key in (
                'secret', 'secretRef', 'secretKeyRef', 'imagePullSecrets')):
            raise ValueError('native_secret_authority_forbidden')
        # A pinned CRD describes optional fields; those are not resource refs.
        if value.get('kind') == 'CustomResourceDefinition':
            return
        for child in value.values():
            reject_secret_authority(child)
    elif isinstance(value, list):
        for child in value:
            reject_secret_authority(child)


def select(objects, kind, name):
    matches = [obj for obj in objects if obj['kind'] == kind and obj['metadata']['name'] == name]
    if len(matches) != 1:
        raise ValueError('native_base_object_missing_or_duplicate')
    return matches[0]


def validate_base(objects):
    unique_objects(objects)
    reject_secret_authority(objects)
    for obj in objects:
        _, kind, namespace, name = object_key(obj)
        if kind in ('Namespace', 'CustomResourceDefinition'):
            if namespace or (kind == 'Namespace' and name != NAMESPACE):
                raise ValueError('native_base_namespace_mismatch')
        elif namespace != NAMESPACE:
            raise ValueError('native_base_namespace_mismatch')
        if kind in ('ClusterRole', 'ClusterRoleBinding', 'ServiceAccount'):
            raise ValueError('native_base_authority_mismatch')
        if kind in ('Role', 'RoleBinding') and name != ROLE_NAME:
            raise ValueError('native_base_authority_mismatch')
        if kind == 'Role' and obj.get('rules') != APP_RULES:
            raise ValueError('native_application_role_drift')
    expected = {
        ('Namespace', NAMESPACE), ('Role', ROLE_NAME), ('RoleBinding', ROLE_NAME),
        ('ConfigMap', 'api-env'), ('ConfigMap', 'mongo-init'), ('ConfigMap', 'identity-code'),
        ('Service', 'api'), ('Service', 'mongo'), ('Service', 'identity'),
        ('Deployment', 'api'), ('Deployment', 'mongo'), ('Deployment', 'identity'),
        ('Job', 'mongo-init'), ('CustomResourceDefinition', 'batchsandboxes.sandbox.opensandbox.io'),
        ('CustomResourceDefinition', 'sandboxes.agents.x-k8s.io'),
        ('CustomResourceDefinition', 'sandboxclaims.extensions.agents.x-k8s.io'),
        ('CustomResourceDefinition', 'sandboxwarmpools.extensions.agents.x-k8s.io'),
        ('CustomResourceDefinition', 'sandboxtemplates.extensions.agents.x-k8s.io'),
    }
    if {(obj['kind'], obj['metadata']['name']) for obj in objects} != expected:
        raise ValueError('native_base_resource_set_mismatch')
    namespace = select(objects, 'Namespace', NAMESPACE)
    if namespace.get('spec'):
        raise ValueError('native_base_namespace_spec_forbidden')
    binding = select(objects, 'RoleBinding', ROLE_NAME)
    if (binding.get('roleRef') != {'apiGroup': 'rbac.authorization.k8s.io',
                                 'kind': 'Role', 'name': ROLE_NAME}
            or binding.get('subjects') != [{'kind': 'ServiceAccount', 'name': 'default',
                                            'namespace': NAMESPACE}]):
        raise ValueError('native_base_binding_mismatch')
    select(objects, 'Role', ROLE_NAME)


def account(name, token):
    return {'apiVersion': 'v1', 'kind': 'ServiceAccount',
            'metadata': {'name': name, 'namespace': NAMESPACE},
            'automountServiceAccountToken': token}


def render(objects, *, enabled=False, cluster=None, node=None, runtime=None,
           runtime_manifest=None, runtime_config=None, tls=None, provider_control=None):
    """Return private manifests plus a credential-free, non-readiness contract.

    Input objects are the generic authenticated fixture. Callers stream the
    private result directly to their owned cluster; only contract is evidence.
    Endpoint manifests are explicit; actual TLS/DNS readiness remains unproven.
    """
    if enabled is False:
        return objects, {'schema': 'migration-native-overlay-v1', 'enabled': False,
                         'runtime_status': 'NOT_RUN', 'compatibility': 'NOT_PROVEN'}
    if enabled is not True:
        raise ValueError('native_explicit_opt_in_required')
    owned_cluster(cluster, node)
    identity = runtime_identity(runtime, runtime_manifest, runtime_config)
    validate_base(objects)
    endpoints, endpoint_contract = render_endpoints(cluster, node, tls, provider_control)
    rendered = [deepcopy(obj) for obj in objects if not (
        obj['kind'] == 'CustomResourceDefinition' and obj['spec']['group'] in NATIVE_GROUPS)]
    native, dependency_contract = dependencies(cluster, node)
    env = select(rendered, 'ConfigMap', 'api-env')['data']
    if env.get('CHRONO_AUTH_MODE') != 'provider':
        raise ValueError('native_synthetic_provider_auth_required')
    if 'CHRONO_AGENT_SANDBOX_TEMPLATE_NETWORK_POLICY' in env:
        raise ValueError('native_existing_template_policy_forbidden')
    if any(env.get('CHRONO_AGENT_SANDBOX_' + suffix) for suffix in (
            'API_SERVER', 'CA_FILE', 'TOKEN_FILE', 'AUTH_MODE', 'IMAGE_PULL_SECRETS',
            'HOME_TEMPLATE', 'HOME_TEMPLATE_UID', 'RUNTIME_CLASS')):
        raise ValueError('native_foreign_runtime_authority')
    env.update({
        'CHRONO_DEPLOYMENT_ID': cluster,
        'CHRONO_SANDBOX_IMAGE': identity['image'],
        'CHRONO_SANDBOX_NAMESPACE': NAMESPACE,
        'CHRONO_AGENT_SANDBOX_NAMESPACE': NAMESPACE,
        'CHRONO_AGENT_SANDBOX_HOME_MODE': 'native_vct_full_home_v2',
        'CHRONO_AGENT_SANDBOX_HOME_STORAGE_CLASS': 'cma-native-rwop',
        'CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB': '4',
        'CHRONO_AGENT_SANDBOX_SERVICE_ACCOUNT': RUNTIME_ACCOUNT,
        'CHRONO_AGENT_SANDBOX_RUNTIME_TRANSPORT': 'direct',
        'CHRONO_AGENT_SANDBOX_TEMPLATE_NETWORK_POLICY': json.dumps(NETWORK_POLICY, separators=(',', ':')),
        'CHRONO_NATIVE_PREPARED_TARGET': '0', 'CHRONO_NATIVE_MACHINE_PREPARED_TARGET': '0',
        'CHRONO_NATIVE_WARM_POOL_TARGET': '0', 'CHRONO_NATIVE_MACHINE_WARM_POOL_TARGET': '0',
        'CHRONO_AGENT_MANAGER_SANDBOX_CPU': '1', 'CHRONO_AGENT_MANAGER_SANDBOX_MEMORY': '2Gi',
        'CHRONO_SANDBOX_CPU': '1', 'CHRONO_SANDBOX_MEMORY': '2Gi',
        'CHRONO_SERVER_PUBLIC_URL': 'https://api.localhost.direct',
        'CHRONO_SERVER_CORS_ORIGIN': 'https://api.localhost.direct',
        'CHRONO_UPSTREAM_BASE_URL': 'https://provider.localhost.direct:8080/v1',
    })
    deployment = select(rendered, 'Deployment', 'api')
    if deployment['spec'].get('replicas') != 1 or deployment['spec'].get('strategy') != {'type': 'Recreate'}:
        raise ValueError('native_single_reader_required')
    spec = deployment['spec']['template']['spec']
    if (len(spec.get('containers', [])) != 1 or spec['containers'][0]['name'] != 'api'
            or spec.get('initContainers') or spec.get('hostNetwork') or spec.get('hostPID')):
        raise ValueError('native_api_container_contract')
    container = spec['containers'][0]
    if (not isinstance(container.get('image'), str) or not IMAGE.fullmatch(container['image'])
            or container.get('imagePullPolicy') != 'Never'):
        raise ValueError('native_api_immutable_image_required')
    if container.get('env') or container.get('envFrom') != [{'configMapRef': {'name': 'api-env'}}]:
        raise ValueError('native_api_environment_override')
    spec.update(serviceAccountName=AM_ACCOUNT, automountServiceAccountToken=True,
                nodeSelector={'kubernetes.io/hostname': node})
    container['startupProbe'] = {'httpGet': {'path': '/livez', 'port': 8080},
                                 'periodSeconds': 2, 'failureThreshold': 90}
    container['env'] = [{'name': name, 'valueFrom': {'fieldRef': {'fieldPath': field}}}
                        for name, field in [('CHRONO_AGENT_MANAGER_POD_NAME', 'metadata.name'),
                                            ('CHRONO_AGENT_MANAGER_POD_NAMESPACE', 'metadata.namespace'),
                                            ('CHRONO_AGENT_MANAGER_POD_UID', 'metadata.uid')]]
    binding = select(rendered, 'RoleBinding', ROLE_NAME)
    binding['subjects'] = [{'kind': 'ServiceAccount', 'name': AM_ACCOUNT, 'namespace': NAMESPACE}]
    rendered.extend([account(AM_ACCOUNT, True), account(RUNTIME_ACCOUNT, False), *native, *endpoints])
    unique_objects(rendered)
    reject_secret_authority(rendered)
    contract = {
        'schema': 'migration-native-overlay-v1', 'enabled': True,
        'cluster': cluster, 'node': node, 'namespace': NAMESPACE,
        'api_image': container['image'],
        'runtime': identity, 'dependencies': dependency_contract,
        'endpoints': endpoint_contract,
        'transport': 'direct', 'home_mode': 'native_vct_full_home_v2',
        'network_policy': deepcopy(NETWORK_POLICY),
        'runtime_status': 'NOT_RUN', 'compatibility': 'NOT_PROVEN',
        'readiness_order': [
            'verify exclusive owned kind node and namespace identity',
            'import and independently bind every immutable image/config',
            'establish real native CRDs; start controller and CSI',
            'observe controller Ready and CSI node registration',
            'start authenticated Mongo and synthetic identity fixture',
            'bind TLS endpoint manifests and owned-cluster DNS; verify hostname and trust',
            'start exact API reader; observe livez, ready, imageID and boot history',
            'admit Agent publicly; observe native Sandbox, Pod, runtime imageID and PVC UIDs',
        ],
        'remaining_runtime_requirements': [
            'actual TLS endpoints and DNS readiness', 'retained-Agent four-stage journey',
            'raw collection/shape snapshots', 'public cleanup and final infrastructure teardown',
        ],
    }
    return rendered, contract
