"""Pure TLS endpoint/DNS rendering for an exclusive disposable kind fixture."""
from copy import deepcopy
import hashlib
from pathlib import Path
import re

from migration_kind_manifests import IDENTITY, NAMESPACE
from migration_native_dependencies import owned_cluster, unique_objects

# SHA-256 pins that identify the fixture's TLS certificate and key PEM files.
CERTIFICATE_PEM_SHA256 = '81367d4b01e92b9172a93d1438b2f412fc7ccdfa59c9624e0e742d6b2effe13a'
SIGNING_PEM_SHA256 = 'f2a383d5e1b7f4d10f1ce4019a4509335e30ba4ba1bccb6deab7d34700b360ff'
TLS_PINS = {'certificate': CERTIFICATE_PEM_SHA256, 'private_key': SIGNING_PEM_SHA256}
DNS_NAMES = {
    'api.localhost.direct': 'api-proxy.cma-migration.svc.cluster.local',
    'provider.localhost.direct': 'provider.cma-migration.svc.cluster.local',
}
CODE_FILES = ('migration_native_provider.py', 'migration_native_provider_server.py',
              'migration_native_proxy.py')


def metadata(name):
    return {'name': name, 'namespace': NAMESPACE}


def render(cluster, node, tls, provider_control):
    owned_cluster(cluster, node)
    if (not isinstance(tls, dict) or set(tls) != set(TLS_PINS)
            or any(not isinstance(tls[key], str) or len(tls[key]) > 65536
                   or hashlib.sha256(tls[key].encode()).hexdigest() != expected
                   for key, expected in TLS_PINS.items())):
        raise ValueError('native_pinned_tls_pair_required')
    if not isinstance(provider_control, str) or not re.fullmatch('[A-Za-z0-9_-]{32,256}', provider_control):
        raise ValueError('native_ephemeral_control_required')
    root = Path(__file__).parent
    code = {name: (root / name).read_text() for name in CODE_FILES}
    private = {'provider.crt': tls['certificate'], 'provider.key': tls['private_key'],
               'provider-control': provider_control}
    objects = [
        {'apiVersion': 'v1', 'kind': 'ConfigMap', 'metadata': metadata('native-code'), 'data': code},
        {'apiVersion': 'v1', 'kind': 'ConfigMap', 'metadata': metadata('native-private'), 'data': private},
    ]
    for name, port, target, script in (
            ('provider', 8080, 8080, 'migration_native_provider_server.py'),
            ('api-proxy', 443, 8443, 'migration_native_proxy.py')):
        container = {
            'name': name, 'image': IDENTITY, 'imagePullPolicy': 'Never',
            'command': ['/usr/local/bin/python3', '-B', '/code/' + script],
            'ports': [{'containerPort': target}],
            'volumeMounts': [{'name': 'private', 'mountPath': '/private', 'readOnly': True},
                             {'name': 'code', 'mountPath': '/code', 'readOnly': True}],
            'resources': {'requests': {'cpu': '25m', 'memory': '32Mi'},
                          'limits': {'cpu': '250m', 'memory': '256Mi'}},
            'readinessProbe': {'tcpSocket': {'port': target}, 'periodSeconds': 2},
        }
        if name == 'provider':
            container['env'] = [
                {'name': 'WS4_PROVIDER_CONTROL_TOKEN', 'valueFrom': {'configMapKeyRef': {
                    'name': 'native-private', 'key': 'provider-control'}}},
                {'name': 'WS4_PROVIDER_TLS_CERT', 'value': '/private/provider.crt'},
                {'name': 'WS4_PROVIDER_TLS_KEY', 'value': '/private/provider.key'},
                {'name': 'WS4_PROVIDER_ALLOW_BOOTSTRAP', 'value': 'true'},
            ]
        objects.extend([
            {'apiVersion': 'v1', 'kind': 'Service', 'metadata': metadata(name),
             'spec': {'selector': {'app': name}, 'ports': [{'port': port, 'targetPort': target}]}},
            {'apiVersion': 'apps/v1', 'kind': 'Deployment', 'metadata': metadata(name),
             'spec': {'replicas': 1, 'selector': {'matchLabels': {'app': name}}, 'template': {
                 'metadata': {'labels': {'app': name}}, 'spec': {
                     'automountServiceAccountToken': False,
                     'nodeSelector': {'kubernetes.io/hostname': node},
                     'containers': [container],
                     'volumes': [{'name': 'private', 'configMap': {'name': 'native-private'}},
                                 {'name': 'code', 'configMap': {'name': 'native-code'}}]}}}},
        ])
    unique_objects(objects)
    return objects, {
        'schema': 'migration-native-endpoints-v1', 'runtime_status': 'NOT_RUN',
        'tls_verification': 'REQUIRED_AT_RUNTIME',
        'required_tls_checks': ['certificate expiration', 'hostname', 'system trust chain',
                                'matching private key', 'actual API/provider response'],
        'certificate_sha256': TLS_PINS['certificate'], 'image': IDENTITY,
        'code_sha256': {name: hashlib.sha256(value.encode()).hexdigest() for name, value in code.items()},
        'dns': deepcopy(DNS_NAMES),
        'provider_control': {'pod_port': 8081, 'bind': '127.0.0.1', 'service_exposed': False,
                             'access': 'owned Pod port-forward; synthetic bearer required'},
        'private_manifest': 'stream only; never include ConfigMap data in evidence',
    }


def bind_dns(configmap, cluster, node):
    """Render a CAS candidate; the runner must preserve UID/resourceVersion."""
    owned_cluster(cluster, node)
    if (not isinstance(configmap, dict) or configmap.get('kind') != 'ConfigMap'
            or configmap.get('apiVersion') != 'v1'
            or configmap.get('metadata', {}).get('namespace') != 'kube-system'
            or configmap['metadata'].get('name') != 'coredns'
            or not configmap['metadata'].get('uid')
            or not configmap['metadata'].get('resourceVersion')):
        raise ValueError('native_owned_coredns_readback_required')
    corefile = configmap.get('data', {}).get('Corefile')
    if (not isinstance(corefile, str) or corefile.count('.:53 {') != 1
            or not re.search(r'^\s*reload\s*$', corefile, flags=re.M)
            or any(name in corefile for name in DNS_NAMES)):
        raise ValueError('native_coredns_template_drift')
    rewrites = ''.join(f'\n    rewrite name exact {source} {target}' for source, target in DNS_NAMES.items())
    result = deepcopy(configmap)
    result['data']['Corefile'] = corefile.replace('.:53 {', '.:53 {' + rewrites, 1)
    return result
