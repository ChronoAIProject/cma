"""Own exactly one disposable Kind cluster and its local child processes."""
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import time
import urllib.request
import uuid

from artifacts import load_image, load_pinned_dependency
from application_fixture import objects as application_objects
from diagnostics import command_outcome
from release_images import load_release_image
from storage_fixture import bind_storage, storage_objects

ROOT = Path(__file__).resolve().parent
NODE = 'kindest/node:v1.34.0@sha256:7416a61b42b1662ca6ca89f02028ac133a309a2a30ba309614e8ec94d976dc5a'
CONTROLLER_URL = 'https://github.com/kubernetes-sigs/agent-sandbox/releases/download/v1.0.2/sandbox-with-extensions.yaml'
CONTROLLER_SHA256 = 'e3dda8359fee2b1bb6f4a9448d7004888286482b17670c581dee976f370fd165'
# Mongo 8.0.13-noble, linux/amd64 manifest (the retained local node used ARM64).
MONGO = 'docker.io/library/mongo@sha256:42545cf0629db918d1a24a2df8bbca6926ed87bfabf59afc85008dd948448cc7'


class Infrastructure:
    def __init__(self, output):
        self.output = Path(output)
        self.cluster = 'cma-ci-' + uuid.uuid4().hex[:12]
        self.node = self.cluster + '-control-plane'
        self.kubeconfig = self.output / 'kubeconfig.private'
        self.created = False
        self.node_id = None
        self.children = []
        self.volumes = []
        self.deadline = time.monotonic() + 3600
        self.stage = 'initialization'
        self.command_outcomes = []
        self.failed_command = None

    def command(self, argv, timeout=60, data=None, *, allow_pending_image=False):
        remaining = self.deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError('infrastructure_deadline')
        # Raw output stays private. Arguments may contain synthetic fixture data.
        path = self.output / ('command-' + uuid.uuid4().hex + '.private.log')
        with path.open('w+') as log:
            started = time.monotonic()
            process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=log, stderr=log,
                                       text=True, start_new_session=True)
            self.children.append(process)
            interrupted = False
            try:
                process.communicate(data, timeout=min(timeout, remaining))
            except BaseException:
                interrupted = True
                self.stop(process)
                raise
            finally:
                log.flush()
                raw = path.read_bytes()
                outcome = command_outcome(self.stage, Path(argv[0]).name,
                    process.returncode, time.monotonic() - started, raw, interrupted)
                inspect_image = argv[:2] == ['docker', 'exec'] and argv[3:5] == ['crictl', 'inspecti'] and len(argv) == 6
                missing_image = False
                if inspect_image and process.returncode == 1 and not interrupted:
                    message = f'no such image "{argv[-1]}" present'
                    # crictl's text formatter can quote and escape the error.
                    missing_image = any(text.encode() in raw for text in (message, json.dumps(message)[1:-1]))
                pending_image = allow_pending_image and missing_image
                if inspect_image:
                    outcome['operation'] = 'cri_image_status'
                if missing_image:
                    outcome['signals'].append('cri_image_absent')
                self.command_outcomes.append(outcome)
                if (process.returncode or interrupted) and not pending_image and self.failed_command is None:
                    self.failed_command = outcome
            if pending_image:
                return None
            if process.returncode:
                raise RuntimeError('fixture_command_failed')
            log.seek(0)
            result = log.read(8 * 1024 * 1024 + 1)
            if len(result) > 8 * 1024 * 1024:
                raise ValueError('fixture_command_output_bound')
            return result

    @staticmethod
    def stop(process):
        if process.poll() is None:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=5)

    def k(self, args, **kwargs):
        return self.command(['kubectl', '--kubeconfig', str(self.kubeconfig),
                             '--context', 'kind-' + self.cluster, '--request-timeout=30s'] + args, **kwargs)

    def cluster_nodes(self):
        return self.command(['docker', 'ps', '-aq', '--no-trunc', '--filter',
                             'label=io.x-k8s.kind.cluster=' + self.cluster]).split()

    def bind_node(self):
        ids = self.cluster_nodes()
        if len(ids) != 1:
            raise ValueError('one_owned_kind_node_required')
        node = json.loads(self.command(['docker', 'inspect', ids[0]]))[0]
        if node['Name'] != '/' + self.node or (self.node_id and node['Id'] != self.node_id):
            raise ValueError('kind_node_identity_changed')
        self.node_id = node['Id']
        for mount in node['Mounts']:
            if mount['Type'] == 'volume' and mount['Name'] not in self.volumes:
                if mount['Name'] in self.previous_volumes:
                    raise ValueError('preexisting_kind_volume')
                self.volumes.append(mount['Name'])
        self.save_state()

    def save_state(self):
        state = {name: getattr(self, name) for name in
                 ['cluster', 'node', 'created', 'node_id', 'volumes', 'previous_volumes']}
        (self.output / 'cluster-state.private.json').write_text(json.dumps(state))

    def wait_for_csi_registration(self):
        deadline = min(self.deadline, time.monotonic() + 45)
        while (remaining := deadline - time.monotonic()) > 0:
            node = json.loads(self.k(['get', 'csinode', self.node, '-o', 'json',
                                      '--ignore-not-found'], timeout=min(remaining, 10)) or '{}')
            drivers = node.get('spec', {}).get('drivers') or []
            if any(driver['name'] == 'hostpath.csi.k8s.io' for driver in drivers):
                return
            time.sleep(max(0, min(deadline - time.monotonic(), 1)))
        raise ValueError('rwop_csi_driver_missing')

    def setup(self, artifacts, revision, run_id, attempt, release_packet=None, *, include_gateway=False,
              split_inventory=False):
        if include_gateway and release_packet:
            raise ValueError('gateway_requires_same_invocation_image')
        self.stage = 'checkout_binding'
        if self.command(['git', 'rev-parse', 'HEAD']).strip() != revision:
            raise ValueError('checkout_revision_mismatch')
        source_tree = self.command(['git', 'rev-parse', 'HEAD^{tree}']).strip()
        self.stage = 'local_docker_preflight'
        endpoint = json.loads(self.command(['docker', 'context', 'inspect']))[0]['Endpoints']['docker']['Host']
        if not endpoint.startswith('unix://'):
            raise ValueError('local_docker_required')
        if self.cluster_nodes():
            raise ValueError('cluster_name_already_exists')
        self.previous_volumes = self.command(['docker', 'volume', 'ls', '-q']).split()
        config = {'kind': 'Cluster', 'apiVersion': 'kind.x-k8s.io/v1alpha4',
                  'nodes': [{'role': 'control-plane', 'image': NODE}],
                  'networking': {'apiServerAddress': '127.0.0.1'}}
        config_path = self.output / 'kind.private.json'
        config_path.write_text(json.dumps(config))
        self.created = True  # Creation intent also owns cleanup of partial setup.
        self.save_state()
        self.stage = 'kind_cluster_create'
        self.command(['kind', 'create', 'cluster', '--name', self.cluster, '--config', str(config_path),
                      '--kubeconfig', str(self.kubeconfig), '--wait', '120s'], timeout=240)
        self.bind_node()
        self.kubeconfig.chmod(0o600)
        self.stage = 'native_platform_binding'
        if self.command(['docker', 'exec', self.node, 'uname', '-m']).strip() != 'x86_64':
            raise ValueError('native_amd64_kernel_required')
        images = {}
        components = ['api', 'runtime', 'runtime-omarchy']
        if include_gateway:
            components.append('cmaeg')
        for component in components:
            self.stage = 'image_' + component.replace('-', '_')
            if release_packet:
                images[component] = load_release_image(self.command, self.node, self.cluster,
                    artifacts, component, release_packet['images'][component])
            else:
                path = Path(artifacts) / f'ci-native-image-{component}-{revision}-{run_id}-{attempt}'
                images[component] = load_image(self.command, self.node, self.cluster, path,
                                               component, revision, run_id, attempt, source_tree=source_tree)
        self.stage = 'mongo_image'
        mongo_image = load_pinned_dependency(self.command, self.node, self.cluster, 'mongo', MONGO)
        self.stage = 'sandbox_controller_manifest'
        with urllib.request.urlopen(CONTROLLER_URL, timeout=30) as response:
            controller = response.read(4 * 1024 * 1024)
        if hashlib.sha256(controller).hexdigest() != CONTROLLER_SHA256:
            raise ValueError('agent_sandbox_manifest_hash')
        self.k(['apply', '-f', '-'], data=controller.decode())
        for index, obj in enumerate(storage_objects()):
            self.stage = 'fixture_storage_' + str(index)
            if obj['kind'] == 'StatefulSet':
                self.stage = 'csi_native_images'
                obj = bind_storage(obj, self.command, self.node, self.cluster)
            self.k(['create', '-f', '-'], data=json.dumps(obj))
        namespace = 'cma-native-am'
        for index, obj in enumerate(application_objects(namespace, split_inventory=split_inventory)):
            self.stage = 'fixture_application_' + str(index)
            self.k(['create', '-f', '-'], data=json.dumps(obj))
        self.stage = 'sandbox_controller_ready'
        self.k(['-n', 'agent-sandbox-system', 'rollout', 'status',
                'deployment/agent-sandbox-controller', '--timeout=180s'], timeout=190)
        self.stage = 'csi_controller_ready'
        self.k(['-n', 'cma-native-storage', 'rollout', 'status',
                'statefulset/csi-hostpathplugin', '--timeout=180s'], timeout=190)
        self.stage = 'csi_driver_registration'
        self.wait_for_csi_registration()
        # Only the owned cluster's DNS is changed. Preserve the other Corefile
        # plugins and send TLS SNI to the same names used by the native client.
        self.stage = 'fixture_dns'
        dns = json.loads(self.k(['-n', 'kube-system', 'get', 'configmap', 'coredns', '-o', 'json']))
        rewrites = ('\n    rewrite name exact provider.localhost.direct provider.cma-native-am.svc.cluster.local\n'
                    '    rewrite name exact storage.localhost.direct storage.cma-native-am.svc.cluster.local\n'
                    '    rewrite name exact api.localhost.direct api-proxy.cma-native-am.svc.cluster.local\n')
        if include_gateway:
            rewrites += '    rewrite name exact identity.localhost.direct identity.cma-native-am.svc.cluster.local\n'
        dns['data']['Corefile'] = dns['data']['Corefile'].replace('.:53 {', '.:53 {' + rewrites, 1)
        self.k(['replace', '-f', '-'], data=json.dumps(dns))
        self.k(['-n', 'kube-system', 'rollout', 'restart', 'deployment/coredns'])
        self.k(['-n', 'kube-system', 'rollout', 'status', 'deployment/coredns', '--timeout=120s'], timeout=130)
        server = json.loads(self.k(['config', 'view', '--minify', '-o', 'json']))['clusters'][0]['cluster']['server']
        return {'run_id': self.cluster, 'namespace': namespace, 'node_name': self.node,
                'kubeconfig_path': str(self.kubeconfig), 'context': 'kind-' + self.cluster, 'server': server,
                'expected_api_image': images['api'], 'api_source': revision,
                **({'expected_eg_image': images['cmaeg']} if include_gateway else {}),
                'mongo_image': mongo_image, 'mongo_registry_pin': MONGO,
                'runtime_images': {'base': images['runtime'], 'omarchy': images['runtime-omarchy']},
                'runtime_sources': {'base': release_packet['images']['runtime']['sourceRevision'] if release_packet else revision,
                                    'omarchy': release_packet['images']['runtime-omarchy']['sourceRevision'] if release_packet else revision}}

    def cleanup(self):
        self.stage = 'owned_cluster_cleanup'
        self.deadline = max(self.deadline, time.monotonic() + 240)
        for process in reversed(self.children):
            self.stop(process)
        if self.created:
            if self.cluster_nodes():
                self.bind_node()
                self.command(['kind', 'delete', 'cluster', '--name', self.cluster,
                              '--kubeconfig', str(self.kubeconfig)], timeout=180)
            if self.cluster_nodes():
                raise RuntimeError('owned_cluster_not_removed')
            remaining = self.command(['docker', 'volume', 'ls', '-q']).split()
            for volume in self.volumes:
                if volume in remaining:
                    if self.command(['docker', 'ps', '-aq', '--filter', 'volume=' + volume]).strip():
                        raise RuntimeError('owned_volume_still_attached')
                    self.command(['docker', 'volume', 'rm', volume])
        if self.created:
            self.created = False
            self.save_state()
        return {'cluster_absent': True, 'children_joined': all(p.poll() is not None for p in self.children)}
