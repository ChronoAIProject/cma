"""Dependency setup for the single owned native migration cluster."""
import base64
from copy import deepcopy
import json
import os
import secrets
import time
from pathlib import Path

from migration_kind_manifests import manifests, NAMESPACE
from migration_native_config import metadata_from_archive, validate_tls
from migration_native_dependencies import dependencies
from migration_native_endpoints import bind_dns
from migration_native_images import import_archive
from migration_native_overlay import render, select
from migration_native_process import require
from migration_native_readiness import job, ready_pod
import migration_agent_documents as documents
import migration_catalog as catalog


def create_cluster(owner):
    context = owner.config['docker_context']
    details = json.loads(owner.command(['docker', 'context', 'inspect', context]))
    require(len(details) == 1 and details[0].get('Name') == context
            and details[0].get('Endpoints', {}).get('docker', {}).get('Host', '').startswith('unix:///'),
            'native_local_docker_required')
    rows = json.loads(owner.docker(['image', 'inspect', owner.config['node']['image']]))
    node = owner.config['node']
    require(len(rows) == 1 and rows[0].get('Os') == 'linux'
            and rows[0].get('Architecture') == node['architecture']
            and node['image'] in rows[0].get('RepoDigests', [])
            and node['config'] in (rows[0].get('Id'), rows[0].get('Descriptor', {}).get(
                'annotations', {}).get('config.digest')), 'native_preloaded_kind_node_required')
    require(not owner.cluster_container_ids(), 'native_cluster_name_already_exists')
    owner.previous_volumes = set(owner.docker(['volume', 'ls', '--format', '{{.Name}}']).decode().splitlines())
    config = {'kind': 'Cluster', 'apiVersion': 'kind.x-k8s.io/v1alpha4',
              'networking': {'apiServerAddress': '127.0.0.1'},
              'nodes': [{'role': 'control-plane', 'image': node['image']}]}
    path = owner.output / 'kind.private.json'
    owner.private_write(path, json.dumps(config).encode())
    owner.creation_intent = True
    owner.record({'type': 'cluster_create_intent', 'cluster': owner.cluster, 'node_image': node})
    owner.command(['env', 'DOCKER_CONTEXT='+context, 'KIND_EXPERIMENTAL_PROVIDER=docker',
                   owner.kind, 'create', 'cluster', '--name', owner.cluster, '--config', str(path),
                   '--kubeconfig', str(owner.kubeconfig), '--wait', '180s'], timeout=240)
    owner.capture_node()
    require(owner.kubeconfig.is_file() and not owner.kubeconfig.is_symlink(), 'native_kubeconfig_missing')
    os.chmod(owner.kubeconfig, 0o600)
    owner.bind_kubeconfig()


def role_job(owner):
    value = job('native-runtime-role', owner.config['artifacts']['old']['image'],
                ['--print-runtime-role'], env={'CHRONO_MONGODB_DATABASE': 'matrix'})
    role = json.loads(owner.run_job(value))
    require(isinstance(role, dict) and role.get('createRole') == 'cma_am_runtime'
            and role.get('roles') == [] and isinstance(role.get('privileges'), list)
            and bool(role['privileges']), 'native_runtime_role_invalid')
    for row in role['privileges']:
        resource = row.get('resource', {})
        require(set(resource) == {'db', 'collection'} and resource['db'] == 'matrix'
                and isinstance(resource['collection'], str) and isinstance(row.get('actions'), list),
                'native_runtime_role_foreign_authority')
    owner.record({'type': 'runtime_role_observed', 'image': owner.config['artifacts']['old'],
                  'privilege_groups': len(role['privileges'])})
    return role


def prepare(owner):
    validate_tls(owner.config['tls'])
    create_cluster(owner)
    for key, image in owner.config['images'].items():
        receipt = import_archive(owner.command, path=owner.config['archives'][image['image']],
                                 expected=image, context=owner.config['docker_context'],
                                 cluster=owner.cluster, owner_node_id=owner.node_id,
                                 kind=owner.kind, enabled=True)
        owner.record({'type': 'image_imported', 'role': key, 'receipt': receipt})
    namespace = {'apiVersion': 'v1', 'kind': 'Namespace', 'metadata': {'name': NAMESPACE}}
    owner.apply([namespace])
    observed = owner.get(['get', 'namespace', NAMESPACE, '-o', 'json'])
    owner.namespace_uid = observed['metadata']['uid']
    require(observed['metadata']['name'] == NAMESPACE and not observed['metadata'].get('deletionTimestamp'),
            'native_namespace_binding_invalid')
    owner.record({'type': 'namespace_created', 'name': NAMESPACE, 'uid': owner.namespace_uid})
    role = role_job(owner)
    identity = {name: secrets.token_urlsafe(32) for name in
                ('client_id', 'client_secret', 'bearer', 'admin_bearer', 'outsider_bearer', 'refresh_token')}
    identity['expires_at'] = int(time.time())+7200
    owner.identity = identity
    owner.provider_token = secrets.token_urlsafe(32)
    for value in [*identity.values(), owner.provider_token]: owner.redactor.remember(value)
    runtime = owner.config['artifacts']['runtime']
    raw_manifest, raw_config = metadata_from_archive(owner.config['archives'][runtime['image']],
                                                     runtime['image'], runtime['config'])
    tls = {key: Path(path).read_text() for key, path in owner.config['tls'].items()}
    for value in tls.values(): owner.redactor.remember(value)
    base = manifests(owner.config['artifacts']['old']['image'], secrets.token_urlsafe(24),
                     secrets.token_urlsafe(24), base64.b64encode(secrets.token_bytes(32)).decode(),
                     owner.config['mongo']['image'], role, runtime['image'],
                     owner.config['identity']['image'], identity)
    objects, contract = render(base, enabled=True, cluster=owner.cluster, node=owner.node,
                               runtime=runtime, runtime_manifest=raw_manifest, runtime_config=raw_config,
                               tls=tls, provider_control=owner.provider_token)
    select(objects, 'ConfigMap', 'api-env')['data'].update({
        'CHRONO_AGENT_MANAGER_REPLICAS_ENABLED': 'true',
        'CHRONO_AGENT_CONTROL_WRITE_COORDINATION_ENABLED': 'true',
        'CHRONO_ADMISSION_CONTROL_WRITES_ENABLED': 'true',
    })
    owner.record({'type': 'native_manifest_contract', 'contract': contract,
                  'coordination': 'ON', 'replicas_enabled': True, 'deployment_replicas': 1,
                  'views': False, 'warmer': False, 'rollups': False})
    objects = catalog.infrastructure(objects, NAMESPACE, secrets.token_urlsafe(24))
    objects = documents.infrastructure(objects, secrets.token_urlsafe(24))
    owner.redactor.collect(objects)
    native, _ = dependencies(owner.cluster, owner.node)
    native_keys = {(r['kind'], r['metadata']['name']) for r in native}
    crds = [o for o in objects if o['kind'] == 'CustomResourceDefinition']
    owner.apply(crds)
    owner.kubectl(['wait', '--for=condition=Established', '--timeout=60s',
                   *['crd/'+o['metadata']['name'] for o in crds]], timeout=70)
    owner.apply([o for o in native if o['kind'] != 'CustomResourceDefinition'])
    for namespace, target in [('agent-sandbox-system', 'deployment/agent-sandbox-controller'),
                              ('cma-native-storage', 'statefulset/csi-hostpathplugin')]:
        owner.kubectl(['-n', namespace, 'rollout', 'status', target, '--timeout=180s'], timeout=190)
    for namespace, label in [('agent-sandbox-system', 'app=agent-sandbox-controller'),
                              ('cma-native-storage', 'app.kubernetes.io/name=csi-hostpathplugin')]:
        pods = owner.get(['-n', namespace, 'get', 'pods', '-l', label, '-o', 'json'])['items']
        require(len(pods) == 1, 'native_dependency_single_pod_required')
        for container in pods[0]['spec']['containers']:
            name = container['name']
            require(name in owner.config['images'], 'native_dependency_container_unknown')
            proof = ready_pod(pods[0], owner.config['images'][name], name, namespace)
            owner.record({'type': 'dependency_ready', 'container': name, 'pod': proof})
    csi = owner.get(['get', 'csinode', owner.node, '-o', 'json'])
    require(csi.get('metadata', {}).get('name') == owner.node
            and any(d.get('name') == 'hostpath.csi.k8s.io' and d.get('nodeID')
                    for d in csi.get('spec', {}).get('drivers', [])), 'native_csi_registration_missing')
    owner.record({'type': 'csi_registered', 'node': owner.node, 'uid': csi['metadata']['uid']})
    owner.api_template = deepcopy(select(objects, 'Deployment', 'api'))
    remaining = [o for o in objects if o['kind'] != 'CustomResourceDefinition'
                 and (o['kind'], o['metadata']['name']) not in native_keys
                 and (o['kind'], o['metadata']['name']) not in (('Deployment', 'api'), ('Namespace', NAMESPACE))]
    owner.apply(remaining)
    owner.kubectl(['-n', NAMESPACE, 'wait', '--for=condition=complete', 'job/mongo-init', '--timeout=180s'], timeout=190)
    for name in ('identity', 'provider', 'api-proxy'):
        owner.kubectl(['-n', NAMESPACE, 'rollout', 'status', 'deployment/'+name, '--timeout=90s'], timeout=100)
        pods = owner.get(['-n', NAMESPACE, 'get', 'pods', '-l', 'app='+name, '-o', 'json'])['items']
        require(len(pods) == 1, 'native_fixture_pod_ambiguous')
        proof = ready_pod(pods[0], owner.config['identity'], name)
        owner.record({'type': 'fixture_ready', 'role': name, 'pod': proof})
        if name == 'provider': owner.provider_pod = proof
    dns = owner.get(['-n', 'kube-system', 'get', 'configmap', 'coredns', '-o', 'json'])
    replacement = bind_dns(dns, owner.cluster, owner.node)
    owner.kubectl(['replace', '-f', '-'], input_text=json.dumps(replacement))
    current = owner.get(['-n', 'kube-system', 'get', 'configmap', 'coredns', '-o', 'json'])
    require(current['metadata']['uid'] == dns['metadata']['uid']
            and current['data'] == replacement['data'], 'native_dns_readback_mismatch')
    owner.record({'type': 'dns_replaced', 'uid': current['metadata']['uid'],
                  'resource_version': current['metadata']['resourceVersion']})
    owner.provider_forward = owner.forward(owner.provider_pod['name'], 8081)
