"""Remove the exact disposable runtime Pod, leaving recovery to Agent Manager."""
import hashlib
import json
import re

from common import require_feature_fixture


def runtime_inventory_authority(fixture, manager=None, allow=None):
    require_feature_fixture(fixture)
    if manager not in ('a', 'b') or type(allow) is not bool:
        raise ValueError('runtime_inventory_authority_scope_required')
    client = fixture.client
    namespace = client.namespace
    account = f'cma-native-am-{manager}'
    binding = account + '-inventory'
    subject = {'kind': 'ServiceAccount', 'name': account, 'namespace': namespace}
    row = client.kjson(['get', 'rolebinding', binding, '-o', 'json'])
    if (row['metadata'].get('name') != binding or row['metadata'].get('namespace') != namespace
            or row.get('roleRef') != {'apiGroup': 'rbac.authorization.k8s.io', 'kind': 'Role',
                                     'name': 'cma-native-am-inventory'}
            or row.get('subjects', []) not in ([], [subject])):
        raise RuntimeError('runtime_inventory_binding_unbound')
    pod = client.kjson(['get', 'pod', f'am-{manager}-0', '-o', 'json'])
    if pod.get('spec', {}).get('serviceAccountName') != account:
        raise RuntimeError('runtime_inventory_process_unbound')
    expected = [subject] if allow else []
    if row.get('subjects', []) != expected:
        row['subjects'] = expected
        client.k(['replace', '-f', '-'], data=json.dumps(row))

    def can_list(am):
        test_account = f'system:serviceaccount:{namespace}:cma-native-am-{am}'
        code, result = client.k(['auth', 'can-i', 'list', 'sandboxes.agents.x-k8s.io',
                                 '--as', test_account], check=False, timeout=5)
        if (code, result.strip()) not in ((0, 'yes'), (1, 'no')):
            raise RuntimeError('runtime_inventory_access_review_failed')
        return code == 0

    peer = 'b' if manager == 'a' else 'a'
    client.wait(lambda: can_list(manager) == allow and can_list(peer), 12,
                'runtime_inventory_authority_not_observed')
    return {'manager': manager, 'allowed': allow, 'peer_allowed': True}


def remove_runtime_pod(self, agentId=None, sandboxId=None, operationId=None, podUid=None):
    require_feature_fixture(self)
    if (any(not isinstance(value, str) or not re.fullmatch(r'[-A-Za-z0-9_]{1,160}', value)
            for value in (agentId, sandboxId, operationId))
            or not isinstance(podUid, str) or not re.fullmatch(r'[a-f0-9-]{36}', podUid)):
        raise ValueError('runtime_loss_lineage_required')
    scope = json.dumps({'_id': agentId, 'sandbox_id': sandboxId, 'status': 'ready',
                        'owner': 'native-am-fixture-owner', 'record.origin.operation_id': operationId})
    if self.mongo(f'print(JSON.stringify(d.agents.findOne({scope}, {{_id:1}})!==null));') is not True:
        raise RuntimeError('runtime_loss_agent_not_owned_ready')
    owned = self.runtimeFor(agentId=agentId)
    if (owned.get('sandbox_id') != sandboxId or owned.get('pod_uid') != podUid or owned.get('ready') is not True
            or owned.get('image') != self.ready['runtime_images']['base']
            or not re.fullmatch(r'[a-z0-9][a-z0-9.-]{0,252}', owned.get('pod_name', ''))):
        raise RuntimeError('runtime_loss_pod_changed')
    # A reused Pod name cannot redirect deletion to the replacement runtime.
    path = f"/api/v1/namespaces/{self.client.namespace}/pods/{owned['pod_name']}"
    options = {'apiVersion': 'v1', 'kind': 'DeleteOptions', 'preconditions': {'uid': podUid}}
    result = self.client.kjson(['delete', '--raw', path, '-f', '-'], data=json.dumps(options), timeout=15)
    if result.get('kind') == 'Pod':
        confirmed = result.get('metadata', {}).get('uid') == podUid
    else:
        confirmed = (result.get('kind') == 'Status' and result.get('status') == 'Success'
                     and result.get('details', {}).get('uid') == podUid)
    if not confirmed:
        raise RuntimeError('runtime_loss_pod_deletion_receipt_unbound')
    self.record('runtime_pod_deletion_accepted', pod_uid_sha256=hashlib.sha256(podUid.encode()).hexdigest())
    return {'accepted': True, 'pod_uid': podUid}
