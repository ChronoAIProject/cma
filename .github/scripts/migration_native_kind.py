"""Concrete exclusive-kind adapter for the retained-Agent migration campaign."""
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import re
import uuid
from urllib.parse import urlencode

from migration_boot_observation import observe_boot
from migration_kind_manifests import NAMESPACE
from migration_native_images import verify_node
from migration_native_kind_setup import prepare
from migration_native_process import Command, Forward, private_directory, require
from migration_native_readiness import TLS_PROBE, image_id_matches, job, projected_boot_list, ready_pod, select_api
import migration_agent_documents as documents
import migration_catalog as catalog


class NativeKind:
    def __init__(self, config, output, journal, *, kind='kind', command_factory=Command,
                 forward_factory=Forward, cluster=None):
        self.config = config
        self.output = private_directory(output)
        self.journal = journal
        self.record = journal.record
        self.redactor = journal.redactor
        self.kind = kind
        self.cluster = cluster or ('cma-matrix-'+uuid.uuid4().hex[:10])
        require(re.fullmatch('cma-matrix-[a-f0-9]{10}', self.cluster), 'native_owned_cluster_required')
        self.node = self.cluster+'-control-plane'
        self.kubeconfig = self.output/'kubeconfig.private'
        self.context = 'kind-'+self.cluster
        self.forward_factory = forward_factory
        self.forwards = []
        self.command = command_factory(self.record, tick=self.pump)
        self.creation_intent = False
        self.node_id = None
        self.previous_volumes = set()
        self.volumes = []
        self.kube_fingerprint = None
        self.server = None
        self.namespace_uid = None
        self.api_template = None
        self.identity = None
        self.provider_token = None
        self.provider_pod = None
        self.provider_forward = None
        self.api_forward = None
        self.binding = None
        self.mongo_pod = None
        self.previous_catalog = None
        self.stopped = False
        self.cleanup_receipt = None

    def pump(self):
        for forward in self.forwards:
            if not forward.closed: forward.pump()

    @staticmethod
    def private_write(path, data):
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, 'wb') as stream:
            stream.write(data);stream.flush();os.fsync(stream.fileno())

    def docker(self, args, **kwargs):
        return self.command(['docker', '--context', self.config['docker_context'], *args], **kwargs)

    def cluster_container_ids(self):
        return self.docker(['ps', '-a', '--filter', 'label=io.x-k8s.kind.cluster='+self.cluster,
                            '--format', '{{.ID}}', '--no-trunc']).decode().splitlines()

    def capture_node(self, *, running=True):
        if running:
            identity = verify_node(self.command, self.config['docker_context'], self.cluster, self.node)
        else:
            details = json.loads(self.docker(['inspect', self.node]))
            require(len(details) == 1 and details[0].get('Name') == '/'+self.node
                    and details[0].get('Config', {}).get('Labels', {}).get('io.x-k8s.kind.cluster') == self.cluster
                    and re.fullmatch('[a-f0-9]{64}', details[0].get('Id', '')), 'native_failed_creation_custody_invalid')
            identity = details[0]['Id']
        require(self.cluster_container_ids() == [identity], 'native_cluster_container_set_changed')
        require(self.node_id in (None, identity), 'native_created_node_replaced')
        details = json.loads(self.docker(['inspect', identity]))[0]
        volumes = [m['Name'] for m in details.get('Mounts', []) if m.get('Type') == 'volume']
        require(len(volumes) == len(set(volumes)) and not set(volumes) & self.previous_volumes,
                'native_node_volume_custody_invalid')
        self.node_id = identity;self.volumes = volumes
        self.record({'type': 'node_created', 'cluster': self.cluster, 'node_container_id': identity,
                     'owned_volumes': volumes})

    def bind_kubeconfig(self):
        raw = self.kubeconfig.read_bytes()
        require(0 < len(raw) <= 65536 and not self.kubeconfig.stat().st_mode & 0o077,
                'native_private_kubeconfig_required')
        self.kube_fingerprint = hashlib.sha256(raw).hexdigest()
        config = self.get(['config', 'view', '--minify', '-o', 'json'])
        require(config.get('current-context') == self.context and len(config.get('clusters', [])) == 1
                and len(config.get('users', [])) == 1 and len(config.get('contexts', [])) == 1,
                'native_kubeconfig_single_context_required')
        cluster = config['clusters'][0];user = config['users'][0];context = config['contexts'][0]
        server = cluster['cluster'].get('server', '')
        require(re.fullmatch(r'https://127\.0\.0\.1:[1-9][0-9]{0,4}', server)
                and int(server.rsplit(':', 1)[1]) <= 65535
                and set(cluster['cluster']) == {'server', 'certificate-authority-data'}
                and set(user['user']) == {'client-certificate-data', 'client-key-data'}
                and context['name'] == self.context and context['context'].get('cluster') == cluster['name']
                and context['context'].get('user') == user['name'], 'native_kubeconfig_authority_invalid')
        self.server = server
        self.record({'type': 'kubeconfig_bound', 'context': self.context, 'server': server,
                     'fingerprint_sha256': self.kube_fingerprint})

    def kubectl(self, args, *, input_text=None, timeout=60):
        require(self.kube_fingerprint is not None and not self.kubeconfig.is_symlink()
                and hashlib.sha256(self.kubeconfig.read_bytes()).hexdigest() == self.kube_fingerprint,
                'native_kubeconfig_changed')
        raw = self.command(['kubectl', '--kubeconfig', str(self.kubeconfig), '--context', self.context,
                            '--request-timeout='+str(max(15, timeout-5))+'s', *args], input_text=input_text, timeout=timeout)
        return raw.decode('utf-8')

    def get(self, args):
        return json.loads(self.kubectl(args))

    def apply(self, objects):
        if objects:
            self.kubectl(['apply', '-f', '-'], input_text=json.dumps({'apiVersion': 'v1', 'kind': 'List', 'items': objects}))

    def run_job(self, manifest):
        name = manifest['metadata']['name'];container = manifest['spec']['template']['spec']['containers'][0]['name']
        self.kubectl(['create', '-f', '-'], input_text=json.dumps(manifest))
        self.kubectl(['-n', NAMESPACE, 'wait', '--for=condition=complete', 'job/'+name, '--timeout=125s'], timeout=135)
        pods = self.get(['-n', NAMESPACE, 'get', 'pods', '-l', 'job-name='+name, '-o', 'json'])['items']
        require(len(pods) == 1 and pods[0]['status']['phase'] == 'Succeeded', 'native_job_completion_invalid')
        states = pods[0]['status'].get('containerStatuses', [])
        image = manifest['spec']['template']['spec']['containers'][0]['image']
        expected = next((row for row in self.config['images'].values() if row['image'] == image), None)
        require(len(states) == 1 and states[0].get('name') == container and states[0].get('restartCount') == 0
                and expected is not None and image_id_matches(states[0].get('imageID'), expected)
                and pods[0]['spec']['containers'][0]['image'] == image
                and states[0].get('state', {}).get('terminated', {}).get('exitCode') == 0,
                'native_job_restart_or_failure')
        metadata = pods[0]['metadata']
        require(metadata.get('uid') and metadata.get('name') and metadata.get('namespace') == NAMESPACE,
                'native_job_pod_identity_missing')
        self.record({'type': 'job_observed', 'job': name, 'pod_name': metadata['name'],
                     'pod_uid': metadata['uid'], 'image': image, 'image_id': states[0]['imageID'],
                     'restarts': 0, 'exit_code': 0})
        return self.kubectl(['-n', NAMESPACE, 'logs', 'pod/'+metadata['name'], '-c', container])

    def forward(self, pod, remote_port):
        require(re.fullmatch('[a-z0-9][a-z0-9.-]{0,252}', pod), 'native_forward_pod_invalid')
        argv = ['kubectl', '--kubeconfig', str(self.kubeconfig), '--context', self.context, '-n', NAMESPACE,
                'port-forward', '--address', '127.0.0.1', 'pod/'+pod, '0:'+str(remote_port)]
        forward = self.forward_factory(argv, remote_port, self.record)
        self.forwards.append(forward)
        return forward.start()

    def prepare(self):
        require(not self.creation_intent and not self.stopped, 'native_kind_prepare_once')
        prepare(self)
        pods = self.get(['-n', NAMESPACE, 'get', 'pods', '-l', 'app=mongo', '-o', 'json'])['items']
        require(len(pods) == 1, 'native_mongo_pod_ambiguous')
        self.mongo_pod = ready_pod(pods[0], self.config['mongo'], 'mongo')
        self.record({'type': 'mongo_bound', 'pod': self.mongo_pod})

    def reader_pods(self):
        value = self.get(['get', '--raw', '/api/v1/namespaces/'+NAMESPACE+'/pods?labelSelector=app%3Dapi'])
        require(not value.get('metadata', {}).get('continue'), 'native_api_list_incomplete')
        return value

    def boot_command(self, args, **kwargs):
        raw = self.kubectl(args, **kwargs)
        if args[-1].startswith('/api/') and 'watch=true' in args[-1]:
            lines = raw.splitlines()
            require(len(lines) <= 2048, 'native_boot_event_bound')
            for line in lines:
                event = json.loads(line);obj = event.get('object', {})
                if event.get('type') in ('ADDED', 'MODIFIED', 'DELETED'):
                    projected = projected_boot_list({'items': [obj]})['items'][0]
                else:
                    projected = {key: obj.get(key) for key in ('code', 'reason')}
                    projected['metadata'] = {'resourceVersion': obj.get('metadata', {}).get('resourceVersion')}
                self.record({'type': 'boot_watch_event', 'event_type': event.get('type'), 'object': projected})
        return raw

    def bind_reader(self, role, stage):
        selected = self.config['artifacts'][role]
        if stage == 'new-write':
            require(self.binding is not None and self.binding['reader']['artifact'] == selected,
                    'native_same_reader_required')
            self.observe_reader(self.binding)
            return {**deepcopy(self.binding), 'stage': stage, 'boot': None}
        before = self.reader_pods()
        self.record({'type': 'pre_boot_list', 'stage': stage, 'pods': projected_boot_list(before)})
        if self.api_forward:
            receipt = self.api_forward.close()
            require(receipt['processes']['process_group_cleanup'], 'native_previous_forward_unjoined')
            self.api_forward = None
        template = deepcopy(self.api_template)
        template['spec']['template']['spec']['containers'][0]['image'] = selected['image']
        try:
            self.apply([template])
            self.kubectl(['-n', NAMESPACE, 'rollout', 'status', 'deployment/api', '--timeout=180s'], timeout=190)
            pod = select_api(self.reader_pods(), selected)
            boot = observe_boot(self.boot_command, NAMESPACE, before, pod, selected['image'], selected['config'])
        except Exception:
            self.record({'type': 'boot_failed', 'stage': stage, 'artifact': selected})
            try:
                query = urlencode({'watch': 'true', 'labelSelector': 'app=api', 'timeoutSeconds': '1',
                                   'resourceVersion': before['metadata']['resourceVersion']})
                self.boot_command(['--request-timeout=15s', 'get', '--raw',
                                   '/api/v1/namespaces/'+NAMESPACE+'/pods?'+query], timeout=20)
                self.record({'type': 'failed_boot_pods', 'stage': stage,
                             'pods': projected_boot_list(self.reader_pods())})
            except Exception:
                self.record({'type': 'failed_boot_diagnostics_unavailable', 'stage': stage})
            raise
        self.api_forward = self.forward(pod['name'], 8080)
        value = {'stage': stage, 'origin': 'http://127.0.0.1:'+str(self.api_forward.port),
                 'reader': {'artifact': selected, 'podName': pod['name'], 'podUid': pod['uid']}, 'boot': boot}
        self.binding = deepcopy(value)
        # This job proves in-cluster DNS, actual system trust, hostnames and
        # responses after reload. No insecure client option or pod exec exists.
        addresses = {name: self.get(['-n', NAMESPACE, 'get', 'service', name, '-o', 'json'])['spec']['clusterIP']
                     for name in ('api-proxy', 'provider')}
        probe = json.loads(self.run_job(job('native-tls-'+stage, self.config['identity']['image'], [], code=TLS_PROBE,
            env={'NATIVE_API_SERVICE_IP': addresses['api-proxy'], 'NATIVE_PROVIDER_SERVICE_IP': addresses['provider']})))
        expected = [{'url': url, 'status': 200, 'verified_tls': True} for url in
                    ('https://api.localhost.direct/livez', 'https://provider.localhost.direct:8080/health')]
        require(probe == {'schema': 'migration-native-tls-probe-v1', 'checks': expected}, 'native_tls_probe_failed')
        self.record({'type': 'tls_probe_observed', 'stage': stage, 'receipt': probe})
        return value

    def session_config(self, binding):
        return {'artifacts': deepcopy(self.config['artifacts']), 'directory': str(self.output),
                'origin': binding['origin'], 'bearer': self.identity['bearer'], 'owner': 'native-am-fixture-owner',
                'providerOrigin': 'http://127.0.0.1:'+str(self.provider_forward.port),
                'providerToken': self.provider_token, 'kubeconfig': str(self.kubeconfig),
                'context': self.context, 'server': self.server, 'namespaceUid': self.namespace_uid}

    def observe_reader(self, binding):
        namespace = self.get(['get', 'namespace', NAMESPACE, '-o', 'json'])
        require(namespace.get('metadata', {}).get('uid') == self.namespace_uid
                and not namespace['metadata'].get('deletionTimestamp'), 'native_namespace_replaced')
        mongo = self.get(['-n', NAMESPACE, 'get', 'pod', self.mongo_pod['name'], '-o', 'json'])
        require(ready_pod(mongo, self.config['mongo'], 'mongo') == self.mongo_pod, 'native_mongo_replaced')
        provider = self.get(['-n', NAMESPACE, 'get', 'pod', self.provider_pod['name'], '-o', 'json'])
        require(ready_pod(provider, self.config['identity'], 'provider') == self.provider_pod,
                'native_provider_replaced')
        pod = select_api(self.reader_pods(), binding['reader']['artifact'])
        require(pod['name'] == binding['reader']['podName'] and pod['uid'] == binding['reader']['podUid'],
                'native_reader_replaced')
        return {'artifact': deepcopy(binding['reader']['artifact']), 'pod': pod}

    def capture_documents(self, stage, binding, session_result):
        selected = binding['reader']['artifact'];reader = self.observe_reader(binding)
        receipt, snapshot = catalog.capture(self, NAMESPACE, self.config['mongo']['image'], stage,
                                             selected['source'], selected['image'])
        self.record({'type': 'catalog_observed', 'receipt': receipt})
        if self.previous_catalog is not None:
            comparison = catalog.compare(self.previous_catalog, snapshot)
            self.record({'type': 'catalog_comparison', 'stage': stage, 'receipt': comparison})
            require(comparison['outcome'] == 'PRESERVED', 'native_catalog_changed')
        self.previous_catalog = snapshot
        observation = documents.capture(self, self.config['mongo']['image'], stage, selected, reader, session_result,
                                        execute_job=self.run_job)
        joined = json.loads(self.command(['node', str(Path(__file__).with_name('migration_agent_document_join.mjs'))],
            input_text=json.dumps({'directory': str(self.output), 'stage': stage, 'selected': selected,
                'owner': 'native-am-fixture-owner', 'session': session_result,
                'snapshot_sha256': observation['raw_sha256']}), timeout=30, max_bytes=1024**2))
        require(joined.get('schema') == 'migration-agent-document-join-v1'
                and joined.get('stage') == stage and joined.get('artifact') == selected
                and joined.get('snapshot_sha256') == observation['raw_sha256']
                and joined.get('session_tail_sha256') == session_result['journal']['tail_sha256']
                and joined.get('outcome') == 'JOURNAL_IDENTITIES_JOINED'
                and joined.get('compatibility') == 'NOT_PROVEN', 'native_document_join_failed')
        return {**observation, 'identity_join': joined}

    def teardown(self):
        if self.stopped: return self.cleanup_receipt
        self.stopped = True
        failures = [];forward_receipts = []
        for forward in reversed(self.forwards):
            try:
                receipt = forward.close();forward_receipts.append(receipt)
                require(receipt['processes']['process_group_cleanup'], 'native_forward_cleanup_failed')
            except Exception:
                failures.append('native_forward_cleanup_failed')
        try:
            if self.creation_intent:
                ids = self.cluster_container_ids()
                if ids:
                    if self.node_id is None: self.capture_node(running=False)
                    require(ids == [self.node_id], 'native_teardown_custody_mismatch')
                    details = json.loads(self.docker(['inspect', self.node_id]))[0]
                    require(details.get('Name') == '/'+self.node
                            and details['Config']['Labels'].get('io.x-k8s.kind.cluster') == self.cluster,
                            'native_teardown_node_mismatch')
                    self.command(['env', 'DOCKER_CONTEXT='+self.config['docker_context'],
                                   'KIND_EXPERIMENTAL_PROVIDER=docker', self.kind, 'delete', 'cluster',
                                   '--name', self.cluster, '--kubeconfig', str(self.kubeconfig)], timeout=180)
                require(not self.cluster_container_ids(), 'native_cluster_remains')
                existing = set(self.docker(['volume', 'ls', '--format', '{{.Name}}']).decode().splitlines())
                for volume in self.volumes:
                    if volume not in existing: continue
                    require(not self.docker(['ps', '-a', '--filter', 'volume='+volume,
                                             '--format', '{{.ID}}']).strip(), 'native_owned_volume_in_use')
                    self.docker(['volume', 'rm', volume])
                remaining = set(self.docker(['volume', 'ls', '--format', '{{.Name}}']).decode().splitlines())
                require(not remaining & set(self.volumes), 'native_owned_volumes_remain')
        except Exception:
            failures.append('native_infrastructure_cleanup_unconfirmed')
        for path in (self.kubeconfig, self.output/'kind.private.json'):
            try: path.unlink(missing_ok=True)
            except OSError: failures.append('native_ephemeral_file_remains')
        self.identity = None;self.provider_token = None;self.api_template = None
        self.cleanup_receipt = {'schema': 'migration-native-kind-finality-v1',
            'outcome': 'OWNED_INFRASTRUCTURE_REMOVED' if not failures else 'INCOMPLETE',
            'cluster': self.cluster, 'node_container_id': self.node_id, 'failures': failures,
            'forwards': forward_receipts, 'owned_volumes': self.volumes}
        self.record({'type': 'kind_finality', 'receipt': self.cleanup_receipt})
        return self.cleanup_receipt
