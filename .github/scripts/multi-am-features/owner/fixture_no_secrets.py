"""Isolated synthetic WS4 inputs; never read, list, create, or repair a Secret."""
from copy import deepcopy
import hashlib
import json
import time
import urllib.parse

from common import Client, write
import manifests
from run import Fixture as OriginalFixture


def synthetic_configmaps(objects):
    """Convert only freshly generated fixture inputs, before Kubernetes access."""
    result = deepcopy(objects)

    def visit(value):
        if isinstance(value, list):
            for item in value:
                visit(item)
        elif isinstance(value, dict):
            if value.get('kind') == 'Secret':
                if value.get('data') or value.get('type') not in (None, 'Opaque'):
                    raise ValueError('unexpected_fixture_secret_encoding')
                value['kind'] = 'ConfigMap'
                value['data'] = value.pop('stringData')
                value.pop('type', None)
            if 'secretRef' in value:
                value['configMapRef'] = value.pop('secretRef')
            if 'secretKeyRef' in value:
                value['configMapKeyRef'] = value.pop('secretKeyRef')
            if 'secret' in value:
                volume = value.pop('secret')
                value['configMap'] = {('name' if k == 'secretName' else k): v
                                      for k, v in volume.items()}
            for child in value.values():
                visit(child)

    visit(result)
    return result


class NoSecretClient(Client):
    environment = None

    def k(self, args, **kwargs):
        if any(x in ('secret', 'secrets') or x.startswith(('secret/', 'secrets/'))
               or '/secrets/' in x or x.endswith('/secrets')
               for arg in args for x in arg.split(',')):
            raise ValueError('secret_access_forbidden')
        return super().k(args, **kwargs)

    def apply(self, objects):
        converted = synthetic_configmaps(objects)
        for obj in converted:
            if obj['kind'] == 'ConfigMap' and obj['metadata']['name'] == 'am-environment':
                data = obj['data']
                self.environment = {
                    'name': obj['metadata']['name'],
                    'database': data['CHRONO_MONGODB_DATABASE'],
                    'database_binding_sha256': hashlib.sha256(
                        data['CHRONO_MONGODB_URI'].encode()).hexdigest(),
                    'coordination_enabled': data['CHRONO_AGENT_MANAGER_REPLICAS_ENABLED'] == 'true',
                    'control_write_coordination_enabled':
                        data['CHRONO_AGENT_CONTROL_WRITE_COORDINATION_ENABLED'] == 'true',
                    'report_views_enabled': data['CHRONO_USAGE_REPORT_VIEWS_ENABLED'] == 'true',
                    'report_warmer_enabled': data['CHRONO_USAGE_REPORT_WARMER_ENABLED'] == 'true',
                    'allocation_rollups_enabled': data['CHRONO_USAGE_ALLOCATION_ROLLUPS_ENABLED'] == 'true',
                }
        return super().apply(converted)


class LocalFixture(OriginalFixture):
    application_deadline = None

    def __init__(self, ready, output):
        super().__init__(ready, output)
        self.client = NoSecretClient(ready, output)
        if self.application_deadline is not None:
            self.client.deadline = self.application_deadline

    def initialize(self):
        super().initialize()
        self.mark_initialization_step('environment_readback')
        expected = {'CHRONO_AGENT_MANAGER_REPLICAS_ENABLED': 'true',
                    'CHRONO_AGENT_CONTROL_WRITE_COORDINATION_ENABLED': 'true',
                    'CHRONO_USAGE_REPORT_VIEWS_ENABLED': 'true',
                    'CHRONO_USAGE_REPORT_WARMER_ENABLED': 'false',
                    'CHRONO_USAGE_ALLOCATION_ROLLUPS_ENABLED': 'false',
                    'CHRONO_DESKTOP_VERIFIED_RUNTIME_IMAGE': self.ready['runtime_images']['omarchy']}
        config = self.client.kjson(['get', 'configmap', 'am-environment', '-o', 'json'])
        observed = {key: config.get('data', {}).get(key) for key in expected}
        if observed != expected:
            raise RuntimeError('actual_am_environment_mismatch')
        scopes = json.loads(config['data']['CHRONO_USAGE_REPORT_VIEW_SCOPES'])
        if scopes != [{'principal': 'native-am-fixture-owner', 'admin': False, 'sandbox_id': None},
                      {'principal': 'native-am-fixture-admin', 'admin': True, 'sandbox_id': None}]:
            raise RuntimeError('actual_usage_view_scope_mismatch')
        write(self.output / 'environment-readback.json', {
            'expected': expected, 'observed': observed, 'verdict': 'PASS',
            'configmap_uid': config['metadata']['uid'], 'secret_resource_access': False})

    def topology(self, **_):
        binding = self.client.environment
        if binding is None:
            raise RuntimeError('created_environment_binding_absent')
        members = self.mongo("print(JSON.stringify(d.cma_agent_manager_members.find({expires_at:{$gt:new Date()}}).toArray().map(x=>({instance_id:x._id,relay_address:x.relay_address,draining:x.draining}))));")
        pods = self.client.kjson(['get', 'pods', '-o', 'json'])['items']
        ams, observed_members, databases = {}, [], []
        for am in ['a', 'b']:
            pod = next((p for p in pods if p['metadata']['name'] == 'am-' + am + '-0'), None)
            if pod is None:
                ams[am] = {'ready': False, 'image': manifests.API, 'pod_uid': None,
                           'instance_id': None, 'container_id': None, 'restarts': None,
                           'started_at': None, 'draining': None}
                continue
            ip = pod.get('status', {}).get('podIP')
            member = next((m for m in members if
                           urllib.parse.urlsplit(m['relay_address']).hostname == ip), None)
            container = pod['spec']['containers'][0]
            refs = [x.get('configMapRef', {}).get('name') for x in container.get('envFrom', [])]
            protected = {'CHRONO_AGENT_MANAGER_REPLICAS_ENABLED',
                         'CHRONO_AGENT_CONTROL_WRITE_COORDINATION_ENABLED',
                         'CHRONO_USAGE_REPORT_VIEWS_ENABLED', 'CHRONO_USAGE_REPORT_WARMER_ENABLED',
                         'CHRONO_USAGE_REPORT_VIEW_SCOPES',
                         'CHRONO_USAGE_ALLOCATION_ROLLUPS_ENABLED', 'CHRONO_DESKTOP_VERIFIED_RUNTIME_IMAGE'}
            if refs != [binding['name']] or any(x['name'].startswith('CHRONO_MONGODB_')
                    or x['name'] in protected for x in container.get('env', [])):
                raise RuntimeError('actual_am_database_environment_drift')
            status = next((s for s in pod.get('status', {}).get('containerStatuses', [])
                           if s.get('name') == container['name']), {})
            running = status.get('state', {}).get('running', {})
            ready = any(x.get('type') == 'Ready' and x.get('status') == 'True'
                        for x in pod.get('status', {}).get('conditions', []))
            if member:
                observed_members.append(member['instance_id'])
                databases.append(binding['database_binding_sha256'])
            ams[am] = {'ready': ready and status.get('ready') is True and bool(running)
                       and member is not None and not member.get('draining', False),
                       'image': container['image'], 'pod_uid': pod['metadata']['uid'],
                       'instance_id': member['instance_id'] if member else None,
                       'pod_ip': ip, 'coordination_enabled': binding['coordination_enabled'],
                       'container_id': status.get('containerID'), 'restarts': status.get('restartCount'),
                       'started_at': running.get('startedAt'),
                       'draining': member.get('draining') if member else None,
                       'database_id': binding['database'], 'origin': self.origins.get(am)}
        return {'coordination_enabled': binding['coordination_enabled'],
                'control_write_coordination_enabled': binding['control_write_coordination_enabled'],
                'report_views_enabled': binding['report_views_enabled'],
                'report_warmer_enabled': binding['report_warmer_enabled'],
                'allocation_rollups_enabled': binding['allocation_rollups_enabled'],
                'shared_database': len(observed_members) == 2 and len(set(observed_members)) == 2
                    and len(set(databases)) == 1,
                'database_binding_basis': 'Created synthetic configuration, matching live Pod references, and two live members observed in the same fixture database',
                'secret_resource_access': False, 'ams': ams,
                'runtime_images': self.ready['runtime_images'],
                'runtime_sources': self.ready['runtime_sources'], 'api_source': manifests.SOURCE}
