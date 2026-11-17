"""Local run17 dependencies with bounded Jobs in place of operator pod exec."""
import base64
import json
from pathlib import Path
import sys
import time

from network import configure_dns, prove_routes

OWNER = Path(__file__).resolve().parent.parent / 'multi-am-features' / 'owner'
sys.path.insert(0, str(OWNER))
from common import write
from fixture_no_secrets import NoSecretClient
from login import login
import legacy_manifests as old
import manifests
from run import Fixture


class JobClient(NoSecretClient):
    def k(self, args, **kwargs):
        if 'exec' in args or any('/exec' in arg for arg in args):
            raise ValueError('operator_pod_exec_forbidden')
        return super().k(args, **kwargs)


class PauseFixture(Fixture):
    def __init__(self, ready, output):
        super().__init__(ready, output)
        self.client = JobClient(ready, output)
        self.client.deadline = time.monotonic() + 14400
        # A serial 100-cycle campaign exceeds the original one-hour fixture JWT.
        expires = int(time.time()) + 14400
        for key in ['bearer', 'admin_bearer', 'outsider_bearer']:
            header, encoded, signature = self.secret[key].split('.')
            claims = json.loads(base64.urlsafe_b64decode(encoded + '=' * (-len(encoded) % 4)))
            claims['exp'] = expires
            payload = base64.urlsafe_b64encode(json.dumps(claims).encode()).decode().rstrip('=')
            self.secret[key] = '.'.join([header, payload, signature])
        self.secret['expires_at'] = expires

    def job(self, script, *, retry=False):
        name = 'autopause-setup-' + str(time.time_ns())
        container = {'name': 'setup', 'image': self.ready['mongo_image'],
            'imagePullPolicy': 'Never', 'command': ['mongosh', '--quiet',
                'mongodb://mongo:27017/admin?directConnection=true&serverSelectionTimeoutMS=3000',
                '--eval', script],
            'env': [{'name': 'MONGO_INITDB_ROOT_USERNAME', 'value': 'root'},
                    {'name': 'MONGO_INITDB_ROOT_PASSWORD', 'valueFrom': {'configMapKeyRef':
                        {'name': 'client-secrets', 'key': 'mongo-root'}}}],
            'resources': {'requests': {'cpu': '25m', 'memory': '64Mi'},
                          'limits': {'cpu': '500m', 'memory': '512Mi'}},
            'volumeMounts': [{'name': 'private', 'mountPath': '/private', 'readOnly': True}]}
        obj = {'apiVersion': 'batch/v1', 'kind': 'Job', 'metadata': old.metadata(name),
            'spec': {'activeDeadlineSeconds': 120, 'backoffLimit': 5 if retry else 0,
                'template': {'spec': {'restartPolicy': 'Never', 'automountServiceAccountToken': False,
                    'containers': [container], 'volumes': [{'name': 'private',
                        'configMap': {'name': 'client-secrets'}}]}}}}
        self.client.apply([obj])
        def complete():
            status = self.client.kjson(['get', 'job', name, '-o', 'json']).get('status', {})
            if any(c['type'] == 'Failed' and c['status'] == 'True' for c in status.get('conditions', [])):
                raise RuntimeError('setup_job_failed_' + name)
            return status.get('succeeded', 0) == 1
        try:
            self.client.wait(complete, 135, 'setup_job_timeout')
            raw = self.client.k(['logs', 'job/' + name, '--limit-bytes=1048576'])
            self.record('setup_job_succeeded', name=name)
            return raw
        finally:
            code, logs = self.client.k(['logs', 'job/' + name, '--all-pods=true',
                                       '--limit-bytes=1048576'], check=False)
            (self.output / (name + '.private.log')).write_text(logs)
            self.record('setup_job_log_retained', name=name, exit=code)
            self.client.k(['delete', 'job', name, '--wait=true', '--timeout=20s'], timeout=25)

    def initialize(self):
        self.certificates()
        self.client.apply(manifests.dependencies(self.secret, self.ready))
        self.client.wait(lambda: self.client.kjson(['get', 'pod', 'am-role', '-o', 'json'])
                         .get('status', {}).get('phase') == 'Succeeded', 120, 'role_not_ready')
        role = json.loads(self.client.k(['logs', 'am-role', '--limit-bytes=524288']))
        write(self.output / 'runtime-role.json', role)
        self.job(old.MONGO_READY, retry=True)
        self.job(old.mongo_role_script(role))
        objects = manifests.managers(self.secret, self.ready)
        environment = next(obj for obj in objects if obj['metadata']['name'] == 'am-environment')
        environment['stringData']['CHRONO_USAGE_REPORT_VIEW_SCOPES'] = json.dumps([
            {'principal': 'native-am-fixture-owner', 'admin': False, 'sandbox_id': None},
            {'principal': 'native-am-fixture-admin', 'admin': True, 'sandbox_id': None},
        ])
        for obj in objects:
            if obj['kind'] == 'StatefulSet':
                container = obj['spec']['template']['spec']['containers'][0]
                # Ownership proves the production API container's startup.
                container['name'] = 'api'
                container['startupProbe'] = {'httpGet': {'path': '/livez', 'port': 8080},
                                             'periodSeconds': 2, 'failureThreshold': 60}
                env = container['env']
                env.extend({'name': name, 'valueFrom': {'fieldRef': {'fieldPath': field}}}
                    for name, field in [('CHRONO_AGENT_MANAGER_POD_NAME', 'metadata.name'),
                                        ('CHRONO_AGENT_MANAGER_POD_NAMESPACE', 'metadata.namespace'),
                                        ('CHRONO_AGENT_MANAGER_POD_UID', 'metadata.uid')])
        # Two managers exercise the selected integration's ownership machinery.
        self.client.apply([o for o in objects if o['kind'] != 'StatefulSet'])
        for am in ['a', 'b']:
            obj = next(o for o in objects if o['kind'] == 'StatefulSet' and o['metadata']['name'] == 'am-' + am)
            self.client.apply([obj])
            self.wait_am(am)
        dns = configure_dns(self.client, self.ready, self.output)
        proof = prove_routes(self.client, self.ready, self.output)
        self.record('runtime_visible_dns_tls_proved', **dns, **proof)
        login(self.client, self.origins['a'], public_host='api.localhost.direct')
        forward = self.client.forward('provider', 8080)
        self.provider_origin = 'https://provider.localhost.direct:' + forward.rsplit(':', 1)[1]
        self.client.wait(lambda: self.provider('GET', '/health').get('ok'), 30, 'provider_not_ready')
        self.record('initialized', operator_pod_exec=False, secret_resource_access=False)

    def runtimeFor(self, agentId=None, **_):
        status, detail = self.client.http(self.origins['a'], 'GET', '/api/v1/agents/' + agentId,
            headers={'Authorization': 'Bearer ' + self.secret['bearer']})
        if status != 200:
            raise RuntimeError('agent_read_' + str(status))
        sandbox = detail['sandbox']['id']
        objects = self.client.kjson(['get', 'sandboxes.agents.x-k8s.io', '-o', 'json'])['items']
        native = [v for v in objects if v['metadata'].get('annotations', {}).get('cma/sandbox') == sandbox]
        if len(native) != 1:
            raise RuntimeError('native_sandbox_identity_missing')
        native = native[0]
        pods = self.client.kjson(['get', 'pods', '-o', 'json'])['items']
        pods = [p for p in pods if any(o['uid'] == native['metadata']['uid']
                for o in p['metadata'].get('ownerReferences', []))]
        claims = self.client.kjson(['get', 'pvc', '-o', 'json'])['items']
        names = {v['persistentVolumeClaim']['claimName'] for p in pods
                 for v in p['spec']['volumes'] if 'persistentVolumeClaim' in v}
        # A stopped Pod is absent; retained claims remain bound to this CR.
        claims = [p for p in claims if p['metadata']['name'] in names or
                  any(o['uid'] == native['metadata']['uid'] for o in p['metadata'].get('ownerReferences', []))]
        return {'sandbox_id': sandbox, 'native_uid': native['metadata']['uid'],
            'home_identity': native['metadata'].get('annotations', {}).get('cma/home-identity'),
            'pvcs': sorted([{'name': p['metadata']['name'], 'uid': p['metadata']['uid'],
                            'volume': p['spec'].get('volumeName')} for p in claims], key=lambda p: p['name']),
            'pods': [{'name': p['metadata']['name'], 'uid': p['metadata']['uid'],
                      'phase': p.get('status', {}).get('phase'),
                      'deleting': bool(p['metadata'].get('deletionTimestamp'))} for p in pods],
            'operating_mode': native['spec'].get('operatingMode'),
            'generation': native['metadata']['generation'],
            'conditions': native.get('status', {}).get('conditions', []),
            'status': detail['sandbox']['status']}

    def captureFailure(self, reason=None, **_):
        self.capture()
        return {'attempted': True, 'reason': reason, 'operator_pod_exec': False}
