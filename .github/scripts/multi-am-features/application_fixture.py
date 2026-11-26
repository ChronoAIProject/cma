"""Render service authority for a disposable two-manager test namespace."""


def objects(namespace):
    rbac = 'rbac.authorization.k8s.io'
    manager = 'cma-native-am'
    authority = {'name': manager, 'namespace': namespace}
    rules = []
    for group, resources, verbs in [
        ('agents.x-k8s.io', ['sandboxes'], ['get', 'list', 'watch', 'create', 'patch', 'delete']),
        ('extensions.agents.x-k8s.io', ['sandboxtemplates', 'sandboxwarmpools', 'sandboxclaims'],
         ['get', 'list', 'create', 'patch', 'delete']),
        ('sandbox.opensandbox.io', ['batchsandboxes'], ['get', 'list', 'delete']),
        ('', ['persistentvolumeclaims'], ['get', 'list', 'create', 'patch', 'delete']),
        ('', ['pods'], ['get', 'list', 'create', 'delete']),
        ('', ['pods/exec'], ['get', 'create']),
        ('snapshot.storage.k8s.io', ['volumesnapshots'], ['get', 'list', 'create', 'delete']),
        ('', ['services', 'events'], ['get', 'list', 'watch']),
    ]:
        rules.append({'apiGroups': [group], 'resources': resources, 'verbs': verbs})
    return [
        {'apiVersion': 'v1', 'kind': 'Namespace', 'metadata': {'name': namespace}},
        {'apiVersion': 'v1', 'kind': 'ServiceAccount', 'metadata': authority},
        {'apiVersion': 'v1', 'kind': 'ServiceAccount',
         'metadata': {'name': 'cma-runtime', 'namespace': namespace}},
        {'apiVersion': rbac + '/v1', 'kind': 'Role', 'metadata': authority, 'rules': rules},
        {'apiVersion': rbac + '/v1', 'kind': 'RoleBinding', 'metadata': authority,
         'subjects': [{'kind': 'ServiceAccount', **authority}],
         'roleRef': {'apiGroup': rbac, 'kind': 'Role', 'name': manager}},
    ]
