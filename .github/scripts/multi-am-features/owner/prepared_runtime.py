"""Join a disposable prepared reservation to a read of its exact Runtime."""
import hashlib
import json
from pathlib import Path
import re

from common import require_feature_fixture


def prepared_runtime_evidence(self, sandboxId=None, operationId=None):
    require_feature_fixture(self, kind='prepared')
    identities = [sandboxId] if operationId is None else [sandboxId, operationId]
    if any(not isinstance(value, str) or not re.fullmatch(r'[A-Za-z0-9_-]{1,128}', value)
           for value in identities):
        raise ValueError('prepared_runtime_lineage_required')
    ownership = ({'state': 'ready', 'record.ownership': None} if operationId is None else {
        'state': 'claimed', 'record.ownership.owner': 'native-am-fixture-owner',
        'record.ownership.operation_id': operationId,
    })
    query = json.dumps({'_id': sandboxId, 'kind': 'reservation', 'machine': {'$ne': True}, **ownership})
    reservation = self.mongo(f'''
      const row = d.native_prepared_sandboxes.findOne({query},
        {{'record.payload.workspaces':1,'record.payload.runtime.provision_generation':1}});
      const payload = row?.record?.payload;
      const workspaces = payload?.workspaces;
      const agents = workspaces?.length === 1 ? workspaces[0].agents : [];
      print(JSON.stringify(agents?.length === 1 ? {{
        agent: agents[0].id, generation: Number(payload.runtime.provision_generation),
      }} : null));
    ''')
    generation = reservation.get('generation') if isinstance(reservation, dict) else None
    if (type(generation) is not int or not 0 < generation < 2**53
            or not isinstance(reservation.get('agent'), str)
            or not re.fullmatch(r'[A-Za-z0-9_-]{1,128}', reservation['agent'])):
        raise RuntimeError('prepared_runtime_reservation_unbound')

    sandboxes = self.client.kjson(['get', 'sandboxes.agents.x-k8s.io', '-o', 'json'])['items']
    matches = [row['metadata']['uid'] for row in sandboxes
               if row['metadata'].get('annotations', {}).get('cma/sandbox') == sandboxId
               and not row['metadata'].get('deletionTimestamp')]
    if len(matches) != 1:
        raise RuntimeError('prepared_runtime_native_sandbox_unbound')
    sandbox_uid = matches[0]
    pods = self.client.kjson(['get', 'pods', '-o', 'json'])['items']
    matches = [pod for pod in pods if not pod['metadata'].get('deletionTimestamp')
               and any(ref.get('uid') == sandbox_uid for ref in pod['metadata'].get('ownerReferences', []))]
    if len(matches) != 1:
        raise RuntimeError('prepared_runtime_native_pod_unbound')
    pod = matches[0]
    metadata = pod['metadata']
    pod_name, pod_uid = metadata['name'], metadata['uid']
    image = self.ready['runtime_images']['base']
    if (not re.fullmatch(r'[a-z0-9][a-z0-9.-]{0,252}', pod_name)
            or not re.fullmatch(r'[a-f0-9-]{36}', pod_uid)
            or not any(container.get('name') == 'runtime' and container.get('image') == image
                       for container in pod['spec']['containers'])
            or not any(container.get('name') == 'runtime' and container.get('ready') is True
                       for container in pod['status'].get('containerStatuses', []))):
        raise RuntimeError('prepared_runtime_pod_not_ready_on_exact_image')
    probe = Path(__file__).with_name('prepared_runtime_ledger.py').read_text()
    evidence = self.client.kjson(['exec', pod_name, '-c', 'runtime', '--', '/usr/bin/python3', '-B', '-c',
                                  probe, sandboxId, reservation['agent'], str(generation)], timeout=8)
    actual = self.client.kjson(['get', 'pod', pod_name, '-o', 'json'])['metadata']
    if actual.get('uid') != pod_uid or actual.get('deletionTimestamp'):
        raise RuntimeError('prepared_runtime_pod_changed_during_read')
    return {**evidence, 'pod_sha256': hashlib.sha256(pod_uid.encode()).hexdigest()}
