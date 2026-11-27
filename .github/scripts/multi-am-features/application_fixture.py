"""Render service authority for a disposable two-manager test namespace."""


def objects(namespace, *, split_inventory=False):
    rbac = 'rbac.authorization.k8s.io'
    manager = 'cma-native-am'
    authority = {'name': manager, 'namespace': namespace}
    accounts = ([{'name': f'{manager}-{am}', 'namespace': namespace} for am in ('a', 'b')]
                if split_inventory else [authority])
    sandbox_verbs = ['get', 'watch', 'create', 'patch', 'delete']
    if not split_inventory:
        sandbox_verbs.append('list')
    rules = []
    for group, resources, verbs in [
        ('agents.x-k8s.io', ['sandboxes'], sandbox_verbs),
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
    resources = [
        {'apiVersion': 'v1', 'kind': 'Namespace', 'metadata': {'name': namespace}},
        *[{'apiVersion': 'v1', 'kind': 'ServiceAccount', 'metadata': account} for account in accounts],
        {'apiVersion': 'v1', 'kind': 'ServiceAccount',
         'metadata': {'name': 'cma-runtime', 'namespace': namespace}},
        {'apiVersion': rbac + '/v1', 'kind': 'Role', 'metadata': authority, 'rules': rules},
        {'apiVersion': rbac + '/v1', 'kind': 'RoleBinding', 'metadata': authority,
         'subjects': [{'kind': 'ServiceAccount', **account} for account in accounts],
         'roleRef': {'apiGroup': rbac, 'kind': 'Role', 'name': manager}},
    ]
    if not split_inventory:
        return resources
    inventory_role = manager + '-inventory'
    resources.append({'apiVersion': rbac + '/v1', 'kind': 'Role',
                      'metadata': {'name': inventory_role, 'namespace': namespace},
                      'rules': [{'apiGroups': ['agents.x-k8s.io'], 'resources': ['sandboxes'], 'verbs': ['list']}]})
    for account in accounts:
        resources.append({'apiVersion': rbac + '/v1', 'kind': 'RoleBinding',
             'metadata': {'name': account['name'] + '-inventory', 'namespace': namespace},
             'subjects': [{'kind': 'ServiceAccount', **account}],
             'roleRef': {'apiGroup': rbac, 'kind': 'Role', 'name': inventory_role}})
    return resources
