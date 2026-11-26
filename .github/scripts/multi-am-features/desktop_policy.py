"""Bind the existing desktop CI seccomp profile, without weakening Pod identity."""
import base64
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def install(infra, image, utility_image):
    if infra.command(['docker', 'exec', infra.node, 'uname', '-m']).strip() != 'x86_64':
        raise ValueError('desktop_profile_requires_native_amd64_kernel')
    kernel = infra.command(['docker', 'exec', infra.node, 'uname', '-r']).strip()
    profile = infra.output / 'desktop-amd64.seccomp.json'
    infra.command(['node', str(ROOT / 'desktop_oci_seccomp.mjs'), str(profile), kernel])
    profile_sha256 = hashlib.sha256(profile.read_bytes()).hexdigest()
    infra.command(['docker', 'exec', infra.node, 'mkdir', '-p', '/var/lib/kubelet/seccomp/cma-ci'])
    infra.command(['docker', 'cp', str(profile), infra.node + ':/var/lib/kubelet/seccomp/cma-ci/desktop-amd64.json'])
    cert = infra.output / 'admission.crt'
    key = infra.output / 'admission.key'
    infra.command(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '2',
                   '-subj', '/CN=desktop-policy.cma-native-am.svc',
                   '-addext', 'subjectAltName=DNS:desktop-policy.cma-native-am.svc',
                   '-keyout', str(key), '-out', str(cert)])
    metadata = {'name': 'desktop-policy', 'namespace': 'cma-native-am'}
    objects = [
        {'apiVersion': 'v1', 'kind': 'ConfigMap', 'metadata': metadata,
         'data': {'tls.crt': cert.read_text(), 'tls.key': key.read_text(),
                  'server.py': (ROOT / 'owner/desktop_admission.py').read_text()}},
        {'apiVersion': 'v1', 'kind': 'Service', 'metadata': metadata,
         'spec': {'selector': {'app': 'desktop-policy'}, 'ports': [{'port': 443, 'targetPort': 8443}]}},
        {'apiVersion': 'v1', 'kind': 'Pod', 'metadata': {**metadata, 'labels': {'app': 'desktop-policy'}},
         'spec': {'automountServiceAccountToken': False, 'restartPolicy': 'Never',
                  'containers': [{'name': 'policy', 'image': utility_image, 'imagePullPolicy': 'Never',
                    'command': ['/usr/bin/python3', '-B', '/tls/server.py'],
                    'env': [{'name': 'DESKTOP_IMAGE', 'value': image}],
                    'resources': {'requests': {'cpu': '25m', 'memory': '32Mi'},
                                  'limits': {'cpu': '250m', 'memory': '128Mi'}},
                    'volumeMounts': [{'name': 'code', 'mountPath': '/tls', 'readOnly': True}],
                    'readinessProbe': {'tcpSocket': {'port': 8443}, 'periodSeconds': 2}}],
                  'volumes': [{'name': 'code', 'configMap': {'name': 'desktop-policy'}}]}},
    ]
    for obj in objects:
        infra.k(['create', '-f', '-'], data=json.dumps(obj))
    infra.k(['-n', 'cma-native-am', 'wait', '--for=condition=Ready', 'pod/desktop-policy',
             '--timeout=120s'], timeout=130)
    webhook = {'apiVersion': 'admissionregistration.k8s.io/v1', 'kind': 'MutatingWebhookConfiguration',
        'metadata': {'name': 'cma-ci-desktop-policy'}, 'webhooks': [{
            'name': 'desktop.multi-am.cma.invalid', 'admissionReviewVersions': ['v1'],
            'sideEffects': 'None', 'failurePolicy': 'Fail', 'timeoutSeconds': 5,
            'namespaceSelector': {'matchLabels': {'kubernetes.io/metadata.name': 'cma-native-am'}},
            'clientConfig': {'caBundle': base64.b64encode(cert.read_bytes()).decode(),
                'service': {'namespace': 'cma-native-am', 'name': 'desktop-policy', 'path': '/mutate', 'port': 443}},
            'rules': [{'operations': ['CREATE'], 'apiGroups': [''], 'apiVersions': ['v1'], 'resources': ['pods']}],
        }]}
    infra.k(['create', '-f', '-'], data=json.dumps(webhook))
    return {'profile_sha256': profile_sha256, 'scope': 'isolated_ci_desktop_only',
            'profile': 'cma-ci/desktop-amd64.json', 'image': image}
