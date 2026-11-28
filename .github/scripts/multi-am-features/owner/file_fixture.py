"""Create one sparse download fixture in the disposable Agent's Runtime."""
import json
import re

from common import require_feature_fixture

OVERSIZE_BYTES = 32 * 1024 * 1024 + 1
SPARSE_FILE = """
import json
import os
import sys

root = os.open('/home', os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
try:
    folder = os.open(sys.argv[1], os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=root)
    try:
        file = os.open('oversize.txt', os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                       0o644, dir_fd=folder)
        try:
            os.fchmod(file, 0o644)
            os.ftruncate(file, int(sys.argv[2]))
            print(json.dumps({'size_bytes': os.fstat(file).st_size}))
        finally:
            os.close(file)
    finally:
        os.close(folder)
finally:
    os.close(root)
"""


def seed_oversize_file(fixture, agentId=None, sandboxId=None, operationId=None, directory=None):
    require_feature_fixture(fixture)
    if (any(not isinstance(value, str) or not re.fullmatch(r'[A-Za-z0-9_-]{1,128}', value)
            for value in (agentId, sandboxId, operationId))
            or not isinstance(directory, str)
            or not re.fullmatch(r'ci-browser-[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}', directory)):
        raise ValueError('oversize_download_lineage_required')
    query = json.dumps({'_id': agentId, 'sandbox_id': sandboxId, 'status': 'ready',
                        'owner': 'native-am-fixture-owner', 'record.origin.operation_id': operationId})
    if fixture.mongo(f'print(JSON.stringify(d.agents.findOne({query}, {{_id:1}})!==null));') is not True:
        raise RuntimeError('oversize_download_agent_not_owned_ready')
    runtime = fixture.runtimeFor(agentId=agentId)
    pod_name = runtime.get('pod_name', '')
    pod_uid = runtime.get('pod_uid', '')
    statuses = runtime.get('container_statuses', [])
    if (runtime.get('agent_id') != agentId or runtime.get('sandbox_id') != sandboxId
            or runtime.get('ready') is not True or runtime.get('provider_status') != 'ready'
            or runtime.get('image') != fixture.ready['runtime_images']['base']
            or not re.fullmatch(r'[a-z0-9][a-z0-9.-]{0,252}', pod_name)
            or not re.fullmatch(r'[a-f0-9-]{36}', pod_uid)
            or not any(row.get('name') == 'runtime' and row.get('ready') is True for row in statuses)):
        raise RuntimeError('oversize_download_runtime_unbound')
    response = fixture.client.kjson(['exec', pod_name, '-c', 'runtime', '--', '/usr/bin/python3', '-B', '-c',
                                     SPARSE_FILE, directory, str(OVERSIZE_BYTES)], timeout=8)
    observed = fixture.client.kjson(['get', 'pod', pod_name, '-o', 'json'])['metadata']
    if observed.get('uid') != pod_uid or observed.get('deletionTimestamp'):
        raise RuntimeError('oversize_download_runtime_changed')
    if response != {'size_bytes': OVERSIZE_BYTES}:
        raise RuntimeError('oversize_download_size_unconfirmed')
    return response
