"""Install an empty document quota baseline before the disposable Managers start."""
import json
from uuid import uuid4

from common import require_feature_fixture
from legacy_manifests import metadata, pod, resource


def initialize_document_quota(fixture):
    require_feature_fixture(fixture)
    if fixture.mongo(
        'if (d.cma_document_artifacts.findOne() !== null || '
        'd.getCollectionNames().includes("cma_document_quotas")) '
        'throw Error("document_quota_fixture_database_not_empty");'
        'print(JSON.stringify(d.createCollection("cma_document_quotas").ok === 1));'
    ) is not True:
        raise RuntimeError('document_quota_collection_not_created')

    # The synthetic storage service starts with an empty in-memory bucket.
    # No Manager or other document writer has started at this point.
    activation = str(uuid4())
    inventory = {'schema_version': 1, 'activation_id': activation,
                 'complete_storage_inventory': True, 'writers_drained': True, 'objects': []}
    client = fixture.client
    client.apply([{'apiVersion': 'v1', 'kind': 'ConfigMap',
                   'metadata': metadata('document-quota-inventory'),
                   'data': {'inventory.json': json.dumps(inventory)}}])
    env = [{'name': name, 'valueFrom': {'secretKeyRef': {'name': 'am-environment', 'key': name}}}
           for name in ('CHRONO_MONGODB_URI', 'CHRONO_MONGODB_DATABASE', 'CHRONO_MONGODB_AUTHORITY_MODE',
                        'CHRONO_AGENT_MANAGER_REPLICAS_ENABLED')]
    for name, args, expected in [
        ('document-quota-prepare', ['--document-quota-prepare', activation], 'preparing'),
        ('document-quota-install', ['--document-quota-install', '/inventory/inventory.json', '--apply'], 'ready'),
    ]:
        job = pod(name, fixture.ready['expected_api_image'], resource('500m', '256Mi', '50m', '32Mi'),
                  args=args, env=env, volumeMounts=[
                      {'name': 'inventory', 'mountPath': '/inventory', 'readOnly': True}])
        job['spec']['volumes'] = [{'name': 'inventory', 'configMap': {'name': 'document-quota-inventory'}}]
        client.apply([job])

        def completed():
            phase = client.kjson(['get', 'pod', name, '-o', 'json'])['status']['phase']
            if phase == 'Failed':
                raise RuntimeError('document_quota_command_failed')
            return phase == 'Succeeded'

        client.wait(completed, 75, 'document_quota_command_timeout')
        status = json.loads(client.k(['logs', name, '--limit-bytes=16384']))
        if status != {'phase': expected, 'dry_run': False, 'installed_rows': 0, 'baseline_rows': 0}:
            raise RuntimeError('document_quota_baseline_unbound')
