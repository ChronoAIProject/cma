"""Bind the disposable CSI controller to verified native dependency images."""
from copy import deepcopy

from artifacts import load_pinned_dependency

CONTAINERS = {'hostpath', 'node-driver-registrar', 'csi-attacher', 'csi-provisioner'}


def bind_storage(manifest, command, node, cluster):
    value = deepcopy(manifest)
    if (value.get('kind') != 'StatefulSet'
            or value.get('metadata', {}).get('name') != 'csi-hostpathplugin'
            or value['metadata'].get('namespace') != 'cma-native-storage'):
        raise ValueError('csi_fixture_identity_mismatch')
    spec = value['spec']['template']['spec']
    containers = spec['containers']
    if (len(containers) != len(CONTAINERS)
            or {c['name'] for c in containers} != CONTAINERS
            or spec.get('initContainers')):
        raise ValueError('csi_fixture_container_mismatch')
    # Each dependency passes the same platform, registry pin, configuration and
    # imported-manifest checks as Mongo before Kubernetes can start the Pod.
    for container in containers:
        container['image'] = load_pinned_dependency(command, node, cluster,
            'storage-' + container['name'], container['image'])
        container['imagePullPolicy'] = 'Never'
    spec['nodeSelector']['kubernetes.io/hostname'] = node
    return value
