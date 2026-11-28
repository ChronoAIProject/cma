import hashlib
import importlib.util
import json
import os
import subprocess
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from cleanup import cleanup
from application_fixture import objects as application_objects
from desktop_policy import install as install_desktop_policy
from infrastructure import Infrastructure
from run_ci import run as run_ci

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / 'owner'))
from common import CommandFailure
from document_quota import initialize_document_quota
from file_fixture import OVERSIZE_BYTES, SPARSE_FILE
from fixture_no_secrets import NoSecretClient, synthetic_configmaps
import manifests
import legacy_manifests
from run import Fixture
from publication_evidence import FEATURES_MONGO_APP, PREPARED_MONGO_APP
from readiness_scaling import observe_never_ready, observe_scale
from runtime_loss import runtime_inventory_authority


class OwnershipTests(unittest.TestCase):
    def test_prepared_lifecycle_read_renews_the_forward_on_the_original_port(self):
        run_id = 'cma-ci-' + 'a' * 12
        origin = 'http://127.0.0.1:12345'
        child = Mock()
        fixture = Fixture.__new__(Fixture)
        fixture.client = Mock(namespace='cma-native-am', forwards={('am-a-0', 8080): (child, origin)})
        fixture.ready = {'fixture': 'prepared', 'run_id': run_id, 'context': 'kind-' + run_id,
                         'server': origin}
        fixture.client.forward.return_value = origin
        for exit_status, outcome in ((None, 'renewed_running'), (1, 'reopened')):
            with self.subTest(exit_status=exit_status):
                fixture.client.forward.reset_mock()
                child.poll.return_value = exit_status
                self.assertEqual(fixture.repairPreparedForward('a'), {'outcome': outcome})
                fixture.client.forward.assert_called_once_with('am-a-0', 8080, renew=True)
        fixture.client.forward.return_value = 'http://127.0.0.1:12346'
        with self.assertRaisesRegex(RuntimeError, 'prepared_forward_origin_changed'):
            fixture.repairPreparedForward('a')

    def test_document_deletion_installs_an_empty_baseline_from_the_manager_image(self):
        run_id = 'cma-ci-' + 'a' * 12
        image = 'localhost/am@sha256:' + 'b' * 64
        client = Mock(namespace='cma-native-am')
        client.kjson.return_value = {'status': {'phase': 'Succeeded'}}
        client.wait.side_effect = lambda predicate, *_: self.assertTrue(predicate())
        client.k.side_effect = [json.dumps({'phase': phase, 'dry_run': False, 'installed_rows': 0,
                                           'baseline_rows': 0}) for phase in ('preparing', 'ready')]
        fixture = SimpleNamespace(client=client, mongo=Mock(return_value=True),
                                  ready={'fixture': 'features', 'run_id': run_id, 'context': 'kind-' + run_id,
                                         'server': 'https://127.0.0.1:12345', 'expected_api_image': image})
        initialize_document_quota(fixture)
        manifest, prepare, install = [call.args[0][0] for call in client.apply.call_args_list]
        inventory = json.loads(manifest['data']['inventory.json'])
        self.assertEqual(inventory, {'schema_version': 1, 'activation_id': inventory['activation_id'],
                                     'complete_storage_inventory': True, 'writers_drained': True, 'objects': []})
        self.assertEqual(prepare['spec']['containers'][0]['args'],
                         ['--document-quota-prepare', inventory['activation_id']])
        self.assertEqual(install['spec']['containers'][0]['args'],
                         ['--document-quota-install', '/inventory/inventory.json', '--apply'])
        for job in (prepare, install):
            self.assertEqual(job['spec']['containers'][0]['image'], image)
            self.assertFalse(job['spec']['automountServiceAccountToken'])
        fixture.ready['fixture'] = 'prepared'
        with self.assertRaisesRegex(ValueError, 'disposable_feature_fixture_required'):
            initialize_document_quota(fixture)
        fixture.mongo.assert_called_once()

    def test_feature_inventory_access_is_independent_for_the_two_managers(self):
        namespace = 'cma-native-am'
        resources = application_objects(namespace, split_inventory=True)
        roles = {row['metadata']['name']: row['rules'] for row in resources if row['kind'] == 'Role'}
        self.assertNotIn('list', roles['cma-native-am'][0]['verbs'])
        self.assertEqual(roles['cma-native-am-inventory'], [{'apiGroups': ['agents.x-k8s.io'],
                                                           'resources': ['sandboxes'], 'verbs': ['list']}])
        manager_bindings = [row for row in resources if row['kind'] == 'RoleBinding'
                            and row['roleRef']['name'] == 'cma-native-am-inventory']
        self.assertEqual([row['subjects'][0]['name'] for row in manager_bindings],
                         ['cma-native-am-a', 'cma-native-am-b'])
        client = Mock(namespace=namespace)
        pod = {'spec': {'serviceAccountName': 'cma-native-am-a'}}
        fixture = SimpleNamespace(client=client, ready={'fixture': 'features', 'run_id': 'cma-ci-0123456789ab',
            'context': 'kind-cma-ci-0123456789ab', 'server': 'https://127.0.0.1:1234'})
        client.wait.side_effect = lambda predicate, *_: self.assertTrue(predicate())
        client.kjson.side_effect = [manager_bindings[0], pod]
        client.k.side_effect = ['', (1, 'no\n'), (0, 'yes\n')]
        self.assertEqual(runtime_inventory_authority(fixture, manager='a', allow=False),
                         {'manager': 'a', 'allowed': False, 'peer_allowed': True})
        replaced = json.loads(client.k.call_args_list[0].kwargs['data'])
        self.assertEqual(replaced['subjects'], [])
        self.assertIn('cma-native-am-a-inventory', replaced['metadata']['name'])
        client.kjson.side_effect = [replaced, pod]
        client.k.side_effect = ['', (0, 'yes\n'), (0, 'yes\n')]
        self.assertTrue(runtime_inventory_authority(fixture, manager='a', allow=True)['allowed'])
        restored = json.loads(client.k.call_args_list[3].kwargs['data'])
        self.assertEqual(restored['subjects'], [{'kind': 'ServiceAccount', 'name': 'cma-native-am-a',
                                                'namespace': namespace}])

    def test_drain_records_readiness_and_liveness_on_the_original_process(self):
        fixture = Fixture.__new__(Fixture)
        fixture.drained, fixture.killed = {}, {}
        fixture.origins = {'a': 'http://127.0.0.1:1234'}
        owner = {'am': 'a', 'epoch': 3, 'pod_uid': 'original-pod', 'instance_id': 'first'}
        fixture.ownerOf = Mock(return_value=owner)
        fixture.client = Mock()
        fixture.client.http.side_effect = [(200, {}), (503, {'reason': 'draining'})]
        fixture.client.http_raw.return_value = (200, b'')
        fixture.client.kjson.return_value = {'metadata': {'uid': 'original-pod'}}
        result = fixture.drainOwner('fixture-agent', owner)
        self.assertEqual({key: result[key] for key in ('readiness', 'liveness', 'drain_reason')},
                         {'readiness': 503, 'liveness': 200, 'drain_reason': True})
        self.assertEqual(fixture.drained, {'a': owner})
        fixture.client.http_raw.assert_called_once_with(fixture.origins['a'], 'GET', '/livez', timeout=3)

        fixture.drained.clear()
        fixture.client.http.side_effect = [(200, {}), (503, {'reason': 'draining'})]
        fixture.client.kjson.return_value = {'metadata': {'uid': 'replacement-pod'}}
        with self.assertRaisesRegex(RuntimeError, 'drained_process_replaced'):
            fixture.drainOwner('fixture-agent', owner)

    def test_fixture_cleanup_failures_preserve_the_main_result_and_remove_the_cluster(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'results'
            args = SimpleNamespace(revision='a' * 40, release_packet=None, packet_sha256=None,
                                   artifacts=Path(directory), run_id='1', attempt='1',
                                   fixture='prepared', output=output)
            image = 'localhost/fixture@sha256:' + 'b' * 64
            ready = {'expected_api_image': image, 'runtime_images': {'base': image, 'omarchy': image}}
            infrastructure = Mock(created=False, stage='two_am_initialized', failed_command=None,
                                  command_outcomes=[])
            infrastructure.setup.return_value = ready
            infrastructure.cleanup.return_value = {'cluster_absent': True, 'children_joined': True}
            private_log = Path(directory) / 'command.private.log'
            private_log.write_text('failed calling webhook https://private.invalid?token=example')
            error = CommandFailure('command-123', 'kubectl', 1, 0.5, private_log)
            error.resource_kind = 'Pod'
            fixture = Mock(thread=None, initialization_step='fixture_dependency_create')
            fixture.initialize.side_effect = error
            fixture.server.server_close.side_effect = OSError('fixture listener')
            fixture.client.deadline = 0
            fixture.client.close.side_effect = TimeoutError('fixture child')

            with patch('run_ci.Infrastructure', return_value=infrastructure), \
                    patch('run_ci.LocalFixture', return_value=fixture), \
                    patch('run_ci.prepare_tls', return_value=output / 'tls'), \
                    patch('run_ci.signal.signal'), patch('run_ci.os.umask'), patch('builtins.print'):
                self.assertEqual(run_ci(args), 1)

            result = json.loads((output / 'summary' / 'result.json').read_text())
            self.assertEqual(result['failure_code'], 'fixture_command_failed')
            self.assertEqual(result['initialization_step'], 'fixture_dependency_create')
            outcome = result['fixture_command_failure']
            self.assertEqual(outcome['stage'], 'fixture_dependency_create')
            self.assertEqual(outcome['program'], 'kubectl')
            self.assertEqual(outcome['returncode'], 1)
            self.assertEqual(outcome['resource_kind'], 'Pod')
            self.assertEqual(outcome['signals'], ['admission_webhook_unreachable'])
            self.assertNotIn('private.invalid', json.dumps(result))
            self.assertEqual(result['fixture_control_cleanup_failure_class'], 'OSError')
            self.assertEqual(result['fixture_child_cleanup_failure_class'], 'TimeoutError')
            self.assertTrue(result['cleanup']['cluster_absent'])
            self.assertEqual(result['verdict'], 'FAIL')
            fixture.server.shutdown.assert_not_called()
            fixture.client.close.assert_called_once_with()
            infrastructure.cleanup.assert_called_once_with()

    def test_csi_registration_waits_for_the_hostpath_driver(self):
        with tempfile.TemporaryDirectory() as directory:
            infra = Infrastructure(directory)
            states = ['', '{"spec":{"drivers":null}}',
                      '{"spec":{"drivers":[{"name":"another-driver"}]}}',
                      '{"spec":{"drivers":[{"name":"hostpath.csi.k8s.io"}]}}']
            with patch.object(infra, 'k', side_effect=states) as k, patch('infrastructure.time.sleep'):
                infra.wait_for_csi_registration()
            self.assertEqual(k.call_count, len(states))

    def test_csi_registration_fails_with_a_named_deadline(self):
        with tempfile.TemporaryDirectory() as directory:
            infra = Infrastructure(directory)
            infra.deadline = 100
            with patch.object(infra, 'k', return_value='{"spec":{"drivers":null}}'), \
                    patch('infrastructure.time.monotonic', side_effect=[80, 80, 99, 100]), \
                    patch('infrastructure.time.sleep'):
                with self.assertRaisesRegex(ValueError, 'rwop_csi_driver_missing'):
                    infra.wait_for_csi_registration()

    def test_startup_witness_rejects_membership_before_ready_and_removes_pod(self):
        changes = []
        env = [{'name': 'POD_IP', 'valueFrom': {'fieldRef': {'fieldPath': 'status.podIP'}}}]
        pod = {'metadata': {'uid': 'witness'}, 'status': {'podIP': '192.0.2.7',
            'containerStatuses': [{'state': {'running': {}}, 'ready': False}]}}

        def query(arguments):
            if arguments[1] == 'nodes':
                return {'items': [{'status': {'allocatable': {'cpu': '3500m'}}}]}
            return pod

        def create(objects):
            obj, = objects
            extra = obj['spec']['containers'][0]['env'][1:]
            self.assertIn({'name': 'CHRONO_AGENT_MANAGER_SANDBOX_CPU', 'value': '4500m'}, extra)
            self.assertEqual(obj['spec']['restartPolicy'], 'Never')

        client = SimpleNamespace(namespace='fixture', apply=create, kjson=query,
            wait=lambda predicate, *_: predicate(), forward=lambda *_: 'http://127.0.0.1:1234',
            http_raw=lambda *_args, **_kwargs: (200,),
            http=lambda *_args, **_kwargs: (503, {'reason': 'startup'}),
            k=lambda arguments, **_: changes.append(arguments),
            forwards={('am-startup-witness', 8080): (None, None)}, stop=lambda _: None)
        fixture = SimpleNamespace(client=client, templates={'b': {'spec': {'template': {'spec': {
            'containers': [{'env': env}], 'restartPolicy': 'Always'}}}}},
            ownerOf=lambda **_: {'am': 'b', 'epoch': 4},
            mongo=lambda _: [{'relay_address': 'http://192.0.2.7:8080'}])
        with self.assertRaisesRegex(RuntimeError, 'startup_witness_joined'):
            observe_never_ready(fixture, 'fixture-agent')
        self.assertEqual(changes[0][:3], ['delete', 'pod', 'am-startup-witness'])
        self.assertEqual(len(env), 1)

    def test_scale_counts_ready_members_and_removes_controller(self):
        actions = []
        image = 'localhost/api@sha256:' + 'a' * 64
        baseline = {'a': {'ready': True, 'pod_uid': 'a'}, 'b': {'ready': True, 'pod_uid': 'b'}}
        replicas = 1

        def command(arguments, **_):
            nonlocal replicas
            actions.append(arguments[:3])
            if arguments[0] == 'patch':
                replicas = json.loads(arguments[-1])['spec']['minReplicas']

        def query(arguments):
            if arguments[1] == 'hpa':
                result = {'spec': {'minReplicas': replicas, 'maxReplicas': replicas}}
                if len(actions) > 1:
                    result['status'] = {'currentReplicas': replicas, 'desiredReplicas': replicas}
                return result
            return {'items': [{'metadata': {'uid': uid, 'name': uid}, 'status': {'podIP': uid,
                'conditions': [{'type': 'Ready', 'status': 'True'}]},
                'spec': {'containers': [{'image': image}]}} for uid in 'abc'[:replicas + 1]]}

        def create(objects):
            obj, = objects
            self.assertEqual(obj['kind'], 'HorizontalPodAutoscaler')
            self.assertEqual(obj['spec']['scaleTargetRef']['name'], 'am-b')

        client = SimpleNamespace(namespace='fixture', k=command, kjson=query, apply=create,
            wait=lambda predicate, *_: predicate(), forward=lambda *_: 'http://127.0.0.1:1234',
            http_raw=lambda *_args, **_kwargs: (200,), forwards={('c', 8080): (None, None)},
            stop=lambda _: actions.append(['stop']))
        fixture = SimpleNamespace(client=client, topology=lambda: {'ams': baseline},
            ready={'expected_api_image': image},
            mongo=lambda _: [{'relay_address': 'http://' + uid + ':8080'} for uid in 'abc'[:replicas + 1]])
        self.assertEqual(observe_scale(fixture)['ready_managers'], [2, 3, 2])
        self.assertEqual(actions[-2:], [['delete', 'hpa', 'two-am-readiness'], ['stop']])
        self.assertEqual(client.forwards, {})

    def test_peer_write_requires_matching_hash_operation_epoch_and_process(self):
        wanted = 'a' * 64
        records = [
            f'DEBUG cma::agent_manager::peer_write: entry request_sha256={wanted} operation="settings" epoch=3',
            f'DEBUG cma::agent_manager::peer_write: entry request_sha256={wanted} operation="settings" epoch=4',
            f'DEBUG cma::agent_manager::peer_write: entry request_sha256={wanted} operation="preferences" epoch=3',
            f'DEBUG cma::unrelated: entry request_sha256={wanted} operation="settings" epoch=3',
        ]
        fixture = Fixture.__new__(Fixture)
        def logs(args, **kwargs):
            self.assertEqual(args, ['logs', 'am-b-0', '--since=20s', '--limit-bytes=1048576'])
            return '\n'.join(records)
        fixture.client = SimpleNamespace(k=logs)
        self.assertEqual(fixture.peerWriteEvidence('b', wanted, 3, 'settings'), {'matches': 1})
        records.append(records[0])
        self.assertEqual(fixture.peerWriteEvidence('b', wanted, 3, 'settings'), {'matches': 2})
        with self.assertRaisesRegex(ValueError, 'peer_write_evidence_scope'):
            fixture.peerWriteEvidence('foreign', wanted, 3, 'settings')

    def test_configuration_dispatch_observes_both_processes_and_counts_duplicates(self):
        target = 'layer2_core::sandbox::settings_dispatch'
        wanted = hashlib.sha256(b'owned-agent').hexdigest()
        def entry(stage, sequence=3, digest=wanted, module=target):
            return f'DEBUG {module}: dispatch agent_sha256={digest} sequence={sequence} stage="{stage}"'

        logs = {'am-a-0': [entry('started'), entry('confirmed'), entry('started', sequence=4)],
                'am-b-0': [entry('started', digest='a' * 64), entry('confirmed', module='other')]}
        fixture = Fixture.__new__(Fixture)
        def read(args, *, timeout):
            self.assertEqual(args[0], 'logs')
            self.assertEqual(args[2:], ['--since=3m', '--limit-bytes=1048576'])
            self.assertEqual(timeout, 5)
            return '\n'.join(logs[args[1]])

        fixture.client = SimpleNamespace(k=read)
        self.assertEqual(fixture.configurationDispatch('owned-agent', 3), {
            'a': {'started': 1, 'confirmed': 1}, 'b': {'started': 0, 'confirmed': 0}})
        logs['am-a-0'].append(entry('started'))
        self.assertEqual(fixture.configurationDispatch('owned-agent', 3)['a']['started'], 2)
        with self.assertRaisesRegex(ValueError, 'configuration_dispatch_lineage_required'):
            fixture.configurationDispatch('owned-agent', True)

    def test_publication_diagnostics_are_scoped_to_the_agent_and_startup_phase(self):
        wanted = hashlib.sha256(b'fixture-agent').hexdigest()

        def entry(failure, step='open_conversation', digest=wanted, target='agent_scheduler_failure'):
            return (f'INFO {target}: Agent publication boundary phase="publication" '
                    f'failure="{failure}" next_step="{step}" agent_sha256={digest}')

        known = entry('known_aborted_cas')
        lease = hashlib.sha256(b'1:fixture-worker').hexdigest()
        committed = (f'INFO agent_scheduler_failure: Prepared Agent Ready publication committed '
                     f'phase="prepared_commit" agent_sha256={wanted} work_lease_sha256={lease} '
                     f'sandbox_revision=12 workspace_revision=9')
        same_sandbox_other_agent = committed.replace(wanted, 'a' * 64).replace('workspace_revision=9',
                                                                                'workspace_revision=8')
        logs = {'am-a-0': [known, known, entry('owner_authority_refused', digest='a' * 64)],
                'am-b-0': [entry('unknown_commit', 'publish_connection'),
                           entry('work_lease_changed', 'publish_deleted'), entry('unknown_commit', target='unrelated'),
                           committed, same_sandbox_other_agent]}
        fixture = Fixture.__new__(Fixture)

        def read(args, *, timeout):
            self.assertEqual(args[0], 'logs')
            self.assertEqual(args[2:], ['--since=12m', '--limit-bytes=2097152'])
            self.assertEqual(timeout, 8)
            return '\n'.join(logs[args[1]])

        fixture.client = SimpleNamespace(k=read)
        self.assertEqual(fixture.publicationEvidence('fixture-agent'), {
            'a': {'known_aborted_cas': {'open_conversation': 2}},
            'b': {'unknown_commit': {'publish_connection': 1}, 'prepared_commit_work_lease_sha256': [lease],
                  'prepared_commit_parent_revisions': [[12, 9]]}})
        with self.assertRaisesRegex(ValueError, 'publication_evidence_lineage_required'):
            fixture.publicationEvidence('../foreign-agent')
        for invalid_revision in ('', '0', '-1', '9007199254740992'):
            logs['am-b-0'].append(committed.replace('workspace_revision=9', f'workspace_revision={invalid_revision}'))
            with self.assertRaisesRegex(RuntimeError, 'prepared_publication_invalid_parent_revisions'):
                fixture.publicationEvidence('fixture-agent')
            logs['am-b-0'].pop()
        logs['am-b-0'].append(entry('unexpected_classification'))
        with self.assertRaisesRegex(RuntimeError, 'publication_evidence_invalid_diagnostic'):
            fixture.publicationEvidence('fixture-agent')

    def test_prepared_publication_conflict_requires_disposable_lineage(self):
        fixture = Fixture.__new__(Fixture)
        run_id = 'cma-ci-' + 'a' * 12
        fixture.client = SimpleNamespace(namespace='cma-native-am')
        fixture.ready = {'fixture': 'prepared', 'run_id': run_id, 'context': 'kind-' + run_id,
                         'server': 'https://127.0.0.1:12345'}
        fixture.mongo = Mock()
        fixture.preparedSchemaRace('inject', 'exact-operation', ['exact-reservation'], 'publication_conflict')
        script = fixture.mongo.call_args.args[0]
        scope, _ = json.JSONDecoder().raw_decode(script.removeprefix('const scope = '))
        self.assertEqual(scope, {'action': 'inject', 'operationId': 'exact-operation',
                                 'reservations': ['exact-reservation'], 'barrier': 'publication_conflict',
                                 'app': PREPARED_MONGO_APP})
        with self.assertRaisesRegex(ValueError, 'prepared_schema_race_lineage_required'):
            fixture.preparedSchemaRace('inject', '../operation', ['exact-reservation'], 'publication_conflict')
        fixture.ready['fixture'] = 'features'
        with self.assertRaisesRegex(ValueError, 'disposable_feature_fixture_required'):
            fixture.preparedSchemaRace('inject', 'exact-operation', ['exact-reservation'], 'publication_conflict')
        fixture.mongo.assert_called_once()

    def test_prepared_reservation_collision_bounds_worktree_reads_without_holding_writes(self):
        fixture = Fixture.__new__(Fixture)
        run_id = 'cma-ci-' + 'a' * 12
        fixture.client = SimpleNamespace(namespace='cma-native-am')
        fixture.ready = {'fixture': 'prepared', 'run_id': run_id, 'context': 'kind-' + run_id,
                         'server': 'https://127.0.0.1:12345'}
        fixture.mongo = Mock()
        fixture.preparedSchemaBarrier(True, reservationConflict=True)
        hold = fixture.mongo.call_args.args[0]
        self.assertIn('"configureFailPoint": "failCommand"', hold)
        self.assertIn('"namespace": "cma_native_am.worktrees"', hold)
        self.assertIn('"failCommands": ["find"]', hold)
        self.assertIn('"blockTimeMS": 12000', hold)
        self.assertIn('"agents"', hold)
        fixture.preparedSchemaBarrier(False, reservationConflict=True)
        self.assertIn('"mode": "off"', fixture.mongo.call_args.args[0])
        with self.assertRaisesRegex(ValueError, 'prepared_schema_race_action_required'):
            fixture.preparedSchemaBarrier(True, reservationConflict=True, ownerFence=True)

    def test_prepared_mongo_command_reports_only_errors_declared_by_its_script(self):
        fixture = Fixture.__new__(Fixture)
        run_id = 'cma-ci-' + 'a' * 12
        fixture.client = SimpleNamespace(namespace='cma-native-am')
        fixture.ready = {'fixture': 'prepared', 'run_id': run_id, 'context': 'kind-' + run_id,
                         'server': 'https://127.0.0.1:12345'}
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / 'command.private.log'
            failure = CommandFailure('command', 'kubectl', 1, .01, log)
            fixture.mongo = Mock(side_effect=failure)
            scope = ('inject', 'exact-operation', ['exact-reservation'], 'publication_conflict')
            log.write_text('Error: prepared_publication_conflict_transaction_changed\n')
            with self.assertRaisesRegex(RuntimeError, '^prepared_publication_conflict_transaction_changed$'):
                fixture.preparedSchemaRace(*scope)
            log.write_text('Error: prepared_synthetic_private_message\n')
            with self.assertRaisesRegex(RuntimeError, '^prepared_schema_race_command_failed$'):
                fixture.preparedSchemaRace(*scope)
            fixture.mongo.side_effect = subprocess.TimeoutExpired(['mongosh', 'private-arguments'], 20)
            with self.assertRaisesRegex(RuntimeError, '^prepared_schema_race_command_timeout$'):
                fixture.preparedSchemaRace(*scope)
            fixture.mongo.side_effect = json.JSONDecodeError('private response', 'private body', 0)
            with self.assertRaisesRegex(RuntimeError, '^prepared_schema_race_response_invalid$'):
                fixture.preparedSchemaRace(*scope)

    def test_prepared_parent_envelope_selector_validates_the_pair_and_holds_sandbox_reads(self):
        fixture = Fixture.__new__(Fixture)
        run_id = 'cma-ci-' + 'a' * 12
        fixture.client = SimpleNamespace(namespace='cma-native-am')
        fixture.ready = {'fixture': 'prepared', 'run_id': run_id, 'context': 'kind-' + run_id,
                         'server': 'https://127.0.0.1:12345'}
        fixture.mongo = Mock()
        fixture.preparedSchemaBarrier(True, parentEnvelope=True)
        self.assertIn('"namespace": "cma_native_am.sandboxes"', fixture.mongo.call_args.args[0])
        fixture.preparedSchemaRace('inject', 'exact-operation', ['exact-reservation'], 'parent_envelope', [4, 3])
        scope, _ = json.JSONDecoder().raw_decode(fixture.mongo.call_args.args[0].removeprefix('const scope = '))
        self.assertEqual(scope['parentVersions'], [4, 3])
        for pair in [None, [True, 3], [3, 4], [5, 1], [1, 3, 4]]:
            with self.assertRaisesRegex(ValueError, 'prepared_parent_envelope_versions_required'):
                fixture.preparedSchemaRace('inject', 'exact-operation', ['exact-reservation'], 'parent_envelope', pair)
        self.assertEqual(fixture.mongo.call_count, 2)

    def test_prepared_pause_observes_the_worktree_read_without_writing_fixture_state(self):
        fixture = Fixture.__new__(Fixture)
        run_id = 'cma-ci-' + 'a' * 12
        fixture.client = SimpleNamespace(namespace='cma-native-am')
        fixture.ready = {'fixture': 'prepared', 'run_id': run_id, 'context': 'kind-' + run_id,
                         'server': 'https://127.0.0.1:12345'}
        fixture.mongo = Mock()
        fixture.preparedSchemaBarrier(True, parentPause=True)
        hold = fixture.mongo.call_args.args[0]
        self.assertIn('"namespace": "cma_native_am.worktrees"', hold)
        self.assertIn('"failCommands": ["find"]', hold)
        self.assertIn('"blockTimeMS": 12000', hold)
        for action in ('sample', 'read'):
            fixture.preparedSchemaRace(action, 'exact-operation', ['exact-reservation'], 'parent_pause')
            scope, _ = json.JSONDecoder().raw_decode(
                fixture.mongo.call_args.args[0].removeprefix('const scope = '))
            self.assertEqual(scope, {'action': action, 'operationId': 'exact-operation',
                                     'reservations': ['exact-reservation'], 'barrier': 'parent_pause',
                                     'app': PREPARED_MONGO_APP})
        for action in ('inject', 'restore'):
            with self.assertRaisesRegex(ValueError, 'prepared_schema_race_lineage_required'):
                fixture.preparedSchemaRace(action, 'exact-operation', ['exact-reservation'], 'parent_pause')
        with self.assertRaisesRegex(ValueError, 'prepared_schema_race_action_required'):
            fixture.preparedSchemaBarrier(True, parentEnvelope=True, parentPause=True)
        self.assertEqual(fixture.mongo.call_count, 3)

    def test_prepared_acknowledgement_fault_requires_a_bound_transaction_proof(self):
        fixture = Fixture.__new__(Fixture)
        run_id = 'cma-ci-' + 'a' * 12
        fixture.client = SimpleNamespace(namespace='cma-native-am')
        fixture.ready = {'fixture': 'prepared', 'run_id': run_id, 'context': 'kind-' + run_id,
                         'server': 'https://127.0.0.1:12345'}
        fixture.mongo = Mock()
        fixture.preparedSchemaBarrier(True, commitAcknowledgement=True)
        hold = fixture.mongo.call_args.args[0]
        self.assertIn('"configureFailPoint": "hangDuringBatchUpdate"', hold)
        self.assertIn('"nss": "cma_native_am.worktrees"', hold)
        fixture.preparedSchemaBarrier(False, commitAcknowledgement=True)
        restore = fixture.mongo.call_args.args[0]
        self.assertIn('configureFailPoint:"failCommand",mode:"off"', restore)
        self.assertIn('profile:0,filter:"unset"', restore)
        arguments = ('exact-operation', ['exact-reservation'], 'commit_acknowledgement')
        fixture.preparedSchemaRace('arm_commit', *arguments)
        proof = {'transaction': 'a' * 64, 'baseline': 7}
        fixture.preparedSchemaRace('verify_commit', *arguments, commitProof=proof)
        scope, _ = json.JSONDecoder().raw_decode(
            fixture.mongo.call_args.args[0].removeprefix('const scope = '))
        self.assertEqual(scope['commitProof'], proof)
        for invalid in (None, {**proof, 'baseline': True}, {**proof, 'transaction': 'foreign'}):
            with self.assertRaisesRegex(ValueError, 'prepared_acknowledgement_proof_required'):
                fixture.preparedSchemaRace('verify_commit', *arguments, commitProof=invalid)
        with self.assertRaisesRegex(ValueError, 'prepared_acknowledgement_proof_required'):
            fixture.preparedSchemaRace('arm_commit', *arguments, commitProof=proof)
        self.assertEqual(fixture.mongo.call_count, 4)

    def test_owner_fence_conflict_targets_only_the_prepared_owner_collection(self):
        fixture = Fixture.__new__(Fixture)
        run_id = 'cma-ci-' + 'a' * 12
        fixture.client = SimpleNamespace(namespace='cma-native-am')
        fixture.ready = {'fixture': 'prepared', 'run_id': run_id, 'context': 'kind-' + run_id,
                         'server': 'https://127.0.0.1:12345'}
        fixture.mongo = Mock()
        fixture.preparedSchemaBarrier(True, ownerFence=True)
        hold = fixture.mongo.call_args.args[0]
        self.assertIn('"namespace": "cma_native_am.cma_agent_manager_owners"', hold)
        self.assertIn('"failCommands": ["update"]', hold)
        self.assertIn('"agents"', hold)
        fixture.preparedSchemaRace('inject', 'exact-operation', ['exact-reservation'], 'owner_fence')
        with self.assertRaisesRegex(ValueError, 'prepared_schema_race_lineage_required'):
            fixture.preparedSchemaRace('read', 'exact-operation', ['exact-reservation'], 'owner_fence')
        fixture.ready['fixture'] = 'features'
        with self.assertRaisesRegex(ValueError, 'disposable_feature_fixture_required'):
            fixture.preparedSchemaBarrier(True, ownerFence=True)
        self.assertEqual(fixture.mongo.call_count, 2)

    def test_prepared_changed_owner_epoch_requires_the_disposable_prepared_fixture(self):
        fixture = Fixture.__new__(Fixture)
        run_id = 'cma-ci-' + 'a' * 12
        fixture.client = SimpleNamespace(namespace='cma-native-am')
        fixture.ready = {'fixture': 'prepared', 'run_id': run_id, 'context': 'kind-' + run_id,
                         'server': 'https://127.0.0.1:12345'}
        fixture.mongo = Mock()
        for action in ('inject', 'read', 'restore'):
            fixture.preparedSchemaRace(action, 'exact-operation', ['exact-reservation'], 'owner_epoch')
            script = fixture.mongo.call_args.args[0]
            scope, _ = json.JSONDecoder().raw_decode(script.removeprefix('const scope = '))
            self.assertEqual(scope, {'action': action, 'operationId': 'exact-operation',
                                     'reservations': ['exact-reservation'], 'barrier': 'owner_epoch',
                                     'app': PREPARED_MONGO_APP})
        fixture.ready['fixture'] = 'features'
        with self.assertRaisesRegex(ValueError, 'disposable_feature_fixture_required'):
            fixture.preparedSchemaRace('inject', 'exact-operation', ['exact-reservation'], 'owner_epoch')
        self.assertEqual(fixture.mongo.call_count, 3)

    def test_artifact_abort_targets_only_the_feature_fixture_insert(self):
        fixture = Fixture.__new__(Fixture)
        run_id = 'cma-ci-' + 'a' * 12
        fixture.client = SimpleNamespace(namespace='cma-native-am')
        fixture.ready = {'fixture': 'features', 'run_id': run_id, 'context': 'kind-' + run_id,
                         'server': 'https://127.0.0.1:12345'}
        fixture.mongo = Mock(return_value={'count': 7})
        self.assertEqual(fixture.artifactPublicationAbort(True), {'count': 7})
        script = fixture.mongo.call_args.args[0]
        command, _ = json.JSONDecoder().raw_decode(script.removeprefix('const result = admin.runCommand('))
        self.assertEqual(command, {'configureFailPoint': 'failCommand', 'mode': {'times': 1}, 'data': {
            'failCommands': ['insert'], 'namespace': 'cma_native_am.agent_artifacts', 'appName': FEATURES_MONGO_APP,
            'errorCode': 112, 'errorLabels': ['TransientTransactionError'],
        }})
        fixture.artifactPublicationAbort(True, 'unknown')
        script = fixture.mongo.call_args.args[0]
        uncertain, _ = json.JSONDecoder().raw_decode(script.removeprefix('const result = admin.runCommand('))
        command['data']['errorLabels'].append('UnknownTransactionCommitResult')
        self.assertEqual(uncertain, command)
        with self.assertRaisesRegex(ValueError, 'artifact_publication_abort_action_required'):
            fixture.artifactPublicationAbort(1)
        with self.assertRaisesRegex(ValueError, 'artifact_publication_abort_action_required'):
            fixture.artifactPublicationAbort(True, 'unspecified')
        fixture.ready['fixture'] = 'prepared'
        with self.assertRaisesRegex(ValueError, 'disposable_feature_fixture_required'):
            fixture.artifactPublicationAbort(True)
        self.assertEqual(fixture.mongo.call_count, 2)

    def test_held_artifact_count_targets_one_feature_transaction_for_the_exact_agent(self):
        fixture = Fixture.__new__(Fixture)
        run_id = 'cma-ci-' + 'a' * 12
        fixture.client = SimpleNamespace(namespace='cma-native-am')
        fixture.ready = {'fixture': 'features', 'run_id': run_id, 'context': 'kind-' + run_id,
                         'server': 'https://127.0.0.1:12345'}
        identity = [{'id': 'synthetic-session'}, '2', 3]
        fixture.mongo = Mock(side_effect=[{'count': 7}, {'entered': True}, [identity], [identity], []])
        self.assertEqual(fixture.artifactPublicationRace('arm', agentId='fixture-agent'), {'count': 7})
        script = fixture.mongo.call_args.args[0]
        command, _ = json.JSONDecoder().raw_decode(script.removeprefix('const result = admin.runCommand('))
        self.assertEqual(command, {'configureFailPoint': 'failCommand', 'mode': {'times': 1}, 'data': {
            'failCommands': ['aggregate'], 'namespace': 'cma_native_am.agent_artifacts',
            'appName': FEATURES_MONGO_APP, 'errorCode': 112, 'errorLabels': ['TransientTransactionError'],
            'blockConnection': True, 'blockTimeMS': 12000,
        }})
        held = fixture.artifactPublicationRace('wait', 7, 'fixture-agent')
        self.assertRegex(held['transaction_sha256'], r'^[a-f0-9]{64}$')
        script = fixture.mongo.call_args.args[0]
        self.assertIn('"command.autocommit": false', script)
        self.assertIn('const target = "fixture-agent"', script)
        self.assertIn('command.pipeline?.[0]?.$match?.agent_id === target', script)
        self.assertEqual(fixture.artifactPublicationRace('sample', agentId='fixture-agent'), held)
        self.assertEqual(fixture.artifactPublicationRace('sample', agentId='fixture-agent'), {'transaction_sha256': None})
        with self.assertRaisesRegex(ValueError, 'publication_evidence_lineage_required'):
            fixture.artifactPublicationRace('arm', agentId='../foreign-agent')
        fixture.ready['fixture'] = 'prepared'
        with self.assertRaisesRegex(ValueError, 'disposable_feature_fixture_required'):
            fixture.artifactPublicationRace('arm', agentId='fixture-agent')
        self.assertEqual(fixture.mongo.call_count, 5)

    def test_access_proposal_abort_targets_only_feature_request_inserts(self):
        fixture = Fixture.__new__(Fixture)
        run_id = 'cma-ci-' + 'a' * 12
        fixture.client = SimpleNamespace(namespace='cma-native-am')
        fixture.ready = {'fixture': 'features', 'run_id': run_id, 'context': 'kind-' + run_id,
                         'server': 'https://127.0.0.1:12345'}
        fixture.mongo = Mock(return_value={'count': 7})
        fixture.accessProposalAbort(True)
        script = fixture.mongo.call_args.args[0]
        command, _ = json.JSONDecoder().raw_decode(script.removeprefix('const result = admin.runCommand('))
        self.assertEqual(command, {'configureFailPoint': 'failCommand', 'mode': {'times': 1}, 'data': {
            'failCommands': ['insert'], 'namespace': 'cma_native_am.agent_access_requests',
            'appName': FEATURES_MONGO_APP, 'errorCode': 112, 'errorLabels': ['TransientTransactionError'],
        }})
        fixture.ready['fixture'] = 'prepared'
        with self.assertRaisesRegex(ValueError, 'disposable_feature_fixture_required'):
            fixture.accessProposalAbort(True)
        fixture.mongo.assert_called_once()

    def test_rename_abort_and_evidence_are_scoped_to_the_feature_fixture_and_agent(self):
        fixture = Fixture.__new__(Fixture)
        run_id = 'cma-ci-' + 'a' * 12
        fixture.client = Mock(namespace='cma-native-am')
        fixture.ready = {'fixture': 'features', 'run_id': run_id, 'context': 'kind-' + run_id,
                         'server': 'https://127.0.0.1:12345'}
        fixture.mongo = Mock(return_value={'count': 7})
        self.assertEqual(fixture.writerPublicationAbort(True), {'count': 7})
        script = fixture.mongo.call_args.args[0]
        command, _ = json.JSONDecoder().raw_decode(script.removeprefix('const result = admin.runCommand('))
        self.assertEqual(command['data'], {'failCommands': ['update'], 'namespace': 'cma_native_am.agents',
            'appName': FEATURES_MONGO_APP, 'errorCode': 112, 'errorLabels': ['TransientTransactionError']})
        digest = hashlib.sha256(b'fixture-agent').hexdigest()
        tag = 'cma::agent_manager::writer_publication:'
        line = f'{tag} transaction aborted agent_sha256={digest} operation="rename" failure="known_aborted_cas"'
        fixture.client.k.side_effect = [
            '\n'.join([line, line.replace(digest, 'f' * 64), line.replace(tag, 'unrelated:'),
                      line.replace('"rename"', '"skill_install"')]),
            line.replace('known_aborted_cas', 'unknown_commit'),
        ]
        self.assertEqual(fixture.writerPublicationEvidence('fixture-agent', 'rename'), {'a': 1, 'b': 0})
        with self.assertRaisesRegex(ValueError, 'writer_publication_operation_required'):
            fixture.writerPublicationEvidence('fixture-agent', 'other')
        fixture.ready['fixture'] = 'prepared'
        with self.assertRaisesRegex(ValueError, 'disposable_feature_fixture_required'):
            fixture.writerPublicationAbort(True)
        with self.assertRaisesRegex(ValueError, 'disposable_feature_fixture_required'):
            fixture.writerPublicationEvidence('fixture-agent', 'rename')

    def test_runtime_loss_evidence_binds_abort_and_committed_recovery_to_each_manager(self):
        fixture = Fixture.__new__(Fixture)
        run_id = 'cma-ci-' + 'a' * 12
        fixture.client = Mock(namespace='cma-native-am')
        fixture.ready = {'fixture': 'features', 'run_id': run_id, 'context': 'kind-' + run_id,
                         'server': 'https://127.0.0.1:12345'}
        digest = hashlib.sha256(b'fixture-sandbox').hexdigest()
        line = ('cma::agent_manager::writer_publication: transaction aborted '
                f'sandbox_sha256={digest} operation="runtime_loss" failure="known_aborted_cas"')
        scheduled = ('cma::sandbox::reconciliation::loss_observation: runtime loss reconciliation outcome '
                     'sandbox_id=fixture-sandbox outcome="recovery_scheduled" elapsed_ms=13')
        fixture.client.k.side_effect = ['\n'.join([
            line.replace(digest, 'f' * 64), line.replace('sandbox_sha256', 'agent_sha256'),
            line.replace('"runtime_loss"', '"retirement"'), scheduled,
        ]), '\n'.join([
            line, scheduled.replace('fixture-sandbox', 'other-sandbox'),
            scheduled.replace('recovery_scheduled', 'superseded'),
            scheduled.replace('loss_observation', 'unrelated'),
        ])]
        self.assertEqual(fixture.runtimeLossEvidence('fixture-sandbox'), {
            'a': {'known_aborts': 0, 'recovery_scheduled': 1},
            'b': {'known_aborts': 1, 'recovery_scheduled': 0},
        })
        self.assertTrue(all('--since=6m' in call.args[0] for call in fixture.client.k.call_args_list))

    def test_runtime_loss_publication_fault_is_one_scoped_write_with_bounded_wait(self):
        fixture = Fixture.__new__(Fixture)
        run_id = 'cma-ci-' + 'a' * 12
        fixture.client = Mock(namespace='cma-native-am')
        fixture.ready = {'fixture': 'features', 'run_id': run_id, 'context': 'kind-' + run_id,
                         'server': 'https://127.0.0.1:12345'}
        scope = {'sandboxId': 'fixture-sandbox', 'manager': 'b'}
        fixture.client.kjson.return_value = {'status': {'podIP': '192.0.2.3'}}
        transaction = [{'id': 'fixture-session'}, '12', 37]
        fixture.mongo = Mock(side_effect=[1, {'count': 7}, {'entered': False}, {'entered': True},
                                          [transaction], [transaction], {'count': 8}, 1])
        self.assertEqual(fixture.runtimeLossPublicationAbort('arm', **scope), {'count': 7})
        self.assertIn('"command.filter._id": "fixture-sandbox"', fixture.mongo.call_args_list[0].args[0])
        script = fixture.mongo.call_args.args[0]
        command, _ = json.JSONDecoder().raw_decode(script.removeprefix('const result = admin.runCommand('))
        self.assertEqual(command['mode'], {'times': 1})
        self.assertEqual(command['data'], {
            'failCommands': ['update'], 'namespace': 'cma_native_am.agents', 'appName': FEATURES_MONGO_APP,
            'errorCode': 112, 'errorLabels': ['TransientTransactionError'],
            'blockConnection': True, 'blockTimeMS': 12000,
        })
        self.assertEqual(fixture.runtimeLossPublicationAbort('wait', 7, **scope), {'transaction_sha256': None})
        self.assertIn('"timesEntered": 8, "maxTimeMS": 4000', fixture.mongo.call_args.args[0])
        fingerprint = hashlib.sha256(json.dumps(transaction, sort_keys=True).encode()).hexdigest()
        self.assertEqual(fixture.runtimeLossPublicationAbort('wait', 7, **scope), {'transaction_sha256': fingerprint})
        sample = fixture.mongo.call_args.args[0]
        for fragment in ['"command.filter._id": "fixture-sandbox"', '"command.update": "agents"',
                         '"command.lsid":row.command.lsid', '"command.txnNumber":row.command.txnNumber']:
            self.assertIn(fragment, sample)
        fixture.client.kjson.assert_called_with(['get', 'pod', 'am-b-0', '-o', 'json'])
        self.assertEqual(fixture.runtimeLossPublicationAbort('sample', **scope), {'transaction_sha256': fingerprint})
        self.assertEqual(fixture.runtimeLossPublicationAbort('disarm', **scope), {'count': 8})
        self.assertIn('"mode": "off"', fixture.mongo.call_args_list[-2].args[0])
        self.assertIn('profile:0,filter:"unset"', fixture.mongo.call_args.args[0])
        with self.assertRaisesRegex(ValueError, 'timed_publication_abort_action_required'):
            fixture.runtimeLossPublicationAbort('wait', True, **scope)
        fixture.ready['fixture'] = 'prepared'
        with self.assertRaisesRegex(ValueError, 'disposable_feature_fixture_required'):
            fixture.runtimeLossPublicationAbort('arm')
        self.assertEqual(fixture.mongo.call_count, 8)

    def test_retirement_purge_writes_are_scoped_to_the_local_feature_fixture(self):
        run_id = 'cma-ci-' + 'a' * 12
        fixture = Fixture.__new__(Fixture)
        fixture.client = SimpleNamespace(namespace='cma-native-am')
        local = {'fixture': 'features', 'run_id': run_id, 'context': 'kind-' + run_id,
                 'server': 'https://127.0.0.1:12345'}
        fixture.ready = local
        script = []
        fixture.mongo = lambda query: script.append(query) or {'armed': True}
        admissions = [{'agentId': 'fixture-agent', 'operationId': 'fixture-create'},
                      {'agentId': 'fixture-child', 'operationId': 'fixture-child-create'}]
        self.assertEqual(fixture.retirementPurge('fixture-agent', 'fixture-sandbox', 'fixture-create', admissions, True),
                         {'armed': True})
        self.assertIn('"arm": true', script[0])
        for unbound in [admissions[1:], [admissions[0], admissions[0]]]:
            with self.assertRaisesRegex(ValueError, 'retirement_purge_fixture_scope_required'):
                fixture.retirementPurge('fixture-agent', 'fixture-sandbox', 'fixture-create', unbound, True)
        for changes in [{'fixture': 'prepared'}, {'context': 'another-kind-context'},
                        {'server': 'https://example.invalid'}, {'run_id': 'foreign'}]:
            fixture.ready = {**local, **changes}
            with self.assertRaisesRegex(ValueError, 'disposable_feature_fixture_required'):
                fixture.retirementPurge('fixture-agent', 'fixture-sandbox', 'fixture-create', admissions, True)
        self.assertEqual(len(script), 1)

    def test_runtime_loss_uses_the_exact_owned_pod_uid_as_a_delete_precondition(self):
        run_id = 'cma-ci-' + 'a' * 12
        pod_uid = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'
        image = 'localhost/base@sha256:' + 'a' * 64
        fixture = Fixture.__new__(Fixture)
        fixture.ready = {'fixture': 'features', 'run_id': run_id, 'context': 'kind-' + run_id,
                         'server': 'https://127.0.0.1:12345', 'runtime_images': {'base': image}}
        mutations = []
        fixture.mongo = lambda query: '"record.origin.operation_id": "fixture-op"' in query
        fixture.runtimeFor = lambda **_: {'sandbox_id': 'fixture', 'pod_uid': pod_uid, 'ready': True,
                                          'image': image, 'pod_name': 'owned-pod'}
        fixture.record = lambda *_args, **_kwargs: None
        def delete(args, *, data, timeout):
            self.assertEqual(args, ['delete', '--raw', '/api/v1/namespaces/cma-native-am/pods/owned-pod', '-f', '-'])
            self.assertEqual(timeout, 15)
            mutations.append(json.loads(data))
            return {'kind': 'Pod', 'metadata': {'uid': pod_uid}}

        fixture.client = SimpleNamespace(namespace='cma-native-am', kjson=delete)
        self.assertEqual(fixture.removeRuntimePod('agent', 'fixture', 'fixture-op', pod_uid),
                         {'accepted': True, 'pod_uid': pod_uid})
        self.assertEqual(mutations, [{'apiVersion': 'v1', 'kind': 'DeleteOptions', 'preconditions': {'uid': pod_uid}}])
        with self.assertRaisesRegex(RuntimeError, 'runtime_loss_pod_changed'):
            fixture.removeRuntimePod('agent', 'other-sandbox', 'fixture-op', pod_uid)
        with self.assertRaisesRegex(RuntimeError, 'runtime_loss_agent_not_owned_ready'):
            fixture.removeRuntimePod('agent', 'fixture', 'foreign-op', pod_uid)
        self.assertEqual(len(mutations), 1)

    def test_oversize_file_is_bound_to_the_disposable_agent_runtime(self):
        run_id = 'cma-ci-' + 'a' * 12
        pod_uid = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'
        directory = 'ci-browser-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'
        image = 'localhost/base@sha256:' + 'a' * 64
        fixture = Fixture.__new__(Fixture)
        fixture.ready = {'fixture': 'features', 'run_id': run_id, 'context': 'kind-' + run_id,
                         'server': 'http://127.0.0.1:12345', 'runtime_images': {'base': image}}
        fixture.mongo = Mock(side_effect=lambda query: '"record.origin.operation_id": "fixture-op"' in query)
        runtime = {'agent_id': 'fixture-agent', 'sandbox_id': 'fixture-sandbox', 'pod_uid': pod_uid,
                   'pod_name': 'fixture-runtime', 'ready': True, 'provider_status': 'ready', 'image': image,
                   'container_statuses': [{'name': 'runtime', 'ready': True}]}
        fixture.runtimeFor = Mock(return_value=runtime)
        fixture.client = Mock(namespace='cma-native-am')
        fixture.client.kjson.side_effect = [
            {'size_bytes': OVERSIZE_BYTES}, {'metadata': {'uid': pod_uid}},
            {'size_bytes': OVERSIZE_BYTES}, {'metadata': {'uid': 'replacement'}},
        ]
        scope = ('fixture-agent', 'fixture-sandbox', 'fixture-op', directory)
        self.assertEqual(fixture.seedOversizeFile(*scope), {'size_bytes': OVERSIZE_BYTES})
        fixture.client.kjson.assert_any_call(
            ['exec', 'fixture-runtime', '-c', 'runtime', '--', '/usr/bin/python3', '-B', '-c',
             SPARSE_FILE, directory, str(OVERSIZE_BYTES)], timeout=8)
        with self.assertRaisesRegex(RuntimeError, 'oversize_download_runtime_changed'):
            fixture.seedOversizeFile(*scope)
        with self.assertRaisesRegex(ValueError, 'oversize_download_lineage_required'):
            fixture.seedOversizeFile(*scope[:3], '../' + directory)
        with self.assertRaisesRegex(RuntimeError, 'oversize_download_agent_not_owned_ready'):
            fixture.seedOversizeFile(*scope[:2], 'other-operation', directory)
        runtime['ready'] = False
        with self.assertRaisesRegex(RuntimeError, 'oversize_download_runtime_unbound'):
            fixture.seedOversizeFile(*scope)
        self.assertEqual(fixture.client.kjson.call_count, 4)

    def test_sparse_download_seed_cannot_follow_symlinks_or_overwrite_existing_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            directory = root / 'fixture'
            directory.mkdir()
            program = SPARSE_FILE.replace("os.open('/home',", f"os.open({str(root)!r},")
            def seed(name):
                return subprocess.run([sys.executable, '-B', '-c', program, name, str(OVERSIZE_BYTES)],
                                      text=True, capture_output=True, timeout=5)

            (root / 'alias').symlink_to(directory, target_is_directory=True)
            target = directory / 'oversize.txt'
            outside = root / 'outside.txt'
            outside.write_text('untouched')
            self.assertNotEqual(seed('alias').returncode, 0)
            target.symlink_to(outside)
            self.assertNotEqual(seed('fixture').returncode, 0)
            self.assertEqual(outside.read_text(), 'untouched')
            target.unlink()
            result = seed('fixture')
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout), {'size_bytes': OVERSIZE_BYTES})
            self.assertEqual(target.stat().st_size, OVERSIZE_BYTES)
            self.assertNotEqual(seed('fixture').returncode, 0)

    def test_cleanup_refuses_unrelated_cluster_before_calling_docker(self):
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / 'cluster-state.private.json').write_text(json.dumps({'cluster': 'production'}))
            with patch.object(Infrastructure, 'command', side_effect=AssertionError('must not execute')):
                with self.assertRaisesRegex(ValueError, 'cluster_state_identity'):
                    cleanup(directory)

    def test_changed_node_identity_never_deleted(self):
        with tempfile.TemporaryDirectory() as directory:
            infra = Infrastructure(directory)
            infra.created = True; infra.node_id = 'old'; infra.previous_volumes = []
            calls = []

            def command(argv, **_):
                calls.append(argv)
                if argv[:3] == ['docker', 'ps', '-aq']:
                    return 'new\n'
                if argv[:2] == ['docker', 'inspect']:
                    return json.dumps([{'Name': '/' + infra.node, 'Id': 'new', 'Mounts': []}])
                raise AssertionError('unexpected mutation')

            infra.command = command
            with self.assertRaisesRegex(ValueError, 'identity_changed'):
                infra.cleanup()
            self.assertFalse(any(argv[:3] == ['kind', 'delete', 'cluster'] for argv in calls))

    def test_synthetic_inputs_use_configmaps_without_changing_source(self):
        objects = [{'kind': 'Secret', 'stringData': {'test': 'generated-fixture-only'}},
                   {'envFrom': [{'secretRef': {'name': 'fixture'}}],
                    'volumes': [{'secret': {'secretName': 'fixture'}}]}]
        converted = synthetic_configmaps(objects)
        self.assertEqual(objects[0]['kind'], 'Secret')
        self.assertEqual(converted[0], {'kind': 'ConfigMap', 'data': {'test': 'generated-fixture-only'}})
        self.assertNotIn('secret', json.dumps(converted))

    def test_secret_resource_access_rejected_before_subprocess(self):
        client = NoSecretClient.__new__(NoSecretClient)
        for arguments in [['get', 'secrets'], ['get', 'pods,secrets'], ['get', 'secret/name']]:
            with self.assertRaisesRegex(ValueError, 'secret_access_forbidden'):
                client.k(arguments)

    def test_two_managers_share_binding_and_have_controller_restart(self):
        ready = {'expected_api_image': 'localhost/api@sha256:' + 'a' * 64, 'api_source': 'b' * 40,
                 'namespace': 'cma-native-am', 'run_id': 'cma-ci-0123456789ab',
                 'mongo_image': 'localhost/multi-am-mongo@sha256:' + 'e' * 64,
                 'runtime_images': {'base': 'localhost/base@sha256:' + 'c' * 64,
                                    'omarchy': 'localhost/desktop@sha256:' + 'd' * 64}}
        objects = synthetic_configmaps(manifests.managers(legacy_manifests.credentials(), ready))
        environment = next(x['data'] for x in objects if x['kind'] == 'ConfigMap')
        self.assertEqual(environment['CHRONO_AGENT_MANAGER_REPLICAS_ENABLED'], 'true')
        self.assertEqual(json.loads(environment['CHRONO_USAGE_REPORT_VIEW_SCOPES']), [
            {'principal': 'native-am-fixture-owner', 'admin': False, 'sandbox_id': None},
            {'principal': 'native-am-fixture-admin', 'admin': True, 'sandbox_id': None},
        ])
        policy = json.loads(environment['CHRONO_TASK_EXPERIENCE_POLICY'])
        self.assertEqual(policy['mode'], 'accounts')
        self.assertEqual(policy['accounts'], ['native-am-fixture-owner'])
        self.assertIn('suggested_followup_admission', policy['capabilities'])
        features = manifests.managers(legacy_manifests.credentials(), {**ready, 'fixture': 'features'})
        features_environment = next(x['stringData'] for x in features if x['kind'] == 'Secret')
        self.assertEqual(features_environment['CHRONO_ADMISSION_CONTROL_WRITES_ENABLED'], 'true')
        self.assertEqual(features_environment['CHRONO_AGENT_SETTINGS_OWNER_RELAY_ENABLED'], 'true')
        for feature in ('QUOTAS', 'SOURCE_COMMITS', 'DELETION'):
            self.assertEqual(features_environment[f'CHRONO_DOCUMENT_{feature}_ENABLED'], 'true')
        self.assertEqual(features_environment['CHRONO_ADMIN_RUNTIME_INVENTORY_ENABLED'], 'true')
        self.assertTrue(features_environment['CHRONO_MONGODB_URI'].endswith('&appName=' + FEATURES_MONGO_APP))
        self.assertEqual(features_environment['CHRONO_AGENT_MANAGER_SANDBOX_CPU'], '1')
        self.assertEqual(features_environment['CHRONO_AGENT_MANAGER_SANDBOX_MEMORY'], '8Gi')
        prepared = synthetic_configmaps(manifests.managers(legacy_manifests.credentials(),
                                                            {**ready, 'fixture': 'prepared'}))
        prepared_environment = next(x['data'] for x in prepared if x['kind'] == 'ConfigMap')
        self.assertNotIn('CHRONO_ADMISSION_CONTROL_WRITES_ENABLED', prepared_environment)
        self.assertNotIn('CHRONO_AGENT_SETTINGS_OWNER_RELAY_ENABLED', prepared_environment)
        self.assertNotIn('CHRONO_ADMIN_RUNTIME_INVENTORY_ENABLED', prepared_environment)
        self.assertEqual(prepared_environment['CHRONO_AGENT_MANAGER_SANDBOX_CPU'], '500m')
        self.assertEqual(prepared_environment['CHRONO_SANDBOX_CPU'], '500m')
        self.assertNotIn('suggested_followup_admission',
                         json.loads(prepared_environment['CHRONO_TASK_EXPERIENCE_POLICY'])['capabilities'])
        managers = [x for x in objects if x['kind'] == 'StatefulSet']
        self.assertEqual({x['metadata']['name'] for x in managers}, {'am-a', 'am-b'})
        for manager in managers:
            self.assertEqual(manager['spec']['replicas'], 1)
            container = manager['spec']['template']['spec']['containers'][0]
            self.assertEqual(container['envFrom'], [{'configMapRef': {'name': 'am-environment'}}])
            self.assertEqual(container['image'], ready['expected_api_image'])
        self.assertEqual(legacy_manifests.MONGO, ready['mongo_image'])
        for missing in ['', 'mongo:latest', 'docker.io/library/mongo@sha256:' + 'e' * 64]:
            with self.assertRaisesRegex(ValueError, 'exact_imported_mongo_required'):
                manifests.configure({**ready, 'mongo_image': missing})


class DesktopPolicyTests(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location('admission_test', ROOT / 'owner/desktop_admission.py')
        with patch.dict(os.environ, {'DESKTOP_IMAGE': 'localhost/desktop@sha256:' + 'a' * 64}):
            self.module = importlib.util.module_from_spec(spec); spec.loader.exec_module(self.module)

    def request(self):
        return {'uid': 'test', 'namespace': 'cma-native-am', 'operation': 'CREATE',
                'resource': {'group': '', 'version': 'v1', 'resource': 'pods'},
                'object': {'spec': {'containers': [{'name': 'runtime', 'image': self.module.IMAGE,
                    'securityContext': {'runAsUser': 10001, 'runAsNonRoot': True,
                        'allowPrivilegeEscalation': False, 'capabilities': {'drop': ['ALL']},
                        'seccompProfile': {'type': 'RuntimeDefault'}}}]}}}

    def test_only_exact_image_and_namespace_receive_localhost_policy(self):
        self.assertIn('patch', self.module.admit(self.request()))
        request = self.request(); request['namespace'] = 'unrelated'
        self.assertNotIn('patch', self.module.admit(request))
        request = self.request(); request['object']['spec']['containers'][0]['image'] = 'another-image'
        self.assertNotIn('patch', self.module.admit(request))

    def test_policy_refuses_weakened_runtime_capabilities(self):
        request = self.request()
        request['object']['spec']['containers'][0]['securityContext']['capabilities']['add'] = ['SYS_ADMIN']
        self.assertFalse(self.module.admit(request)['allowed'])

    def test_api_server_calls_fail_closed_webhook_only_for_exact_desktop_image(self):
        with tempfile.TemporaryDirectory() as directory:
            infrastructure = Mock(output=Path(directory), node='disposable-kind-node')
            for name in ('desktop-amd64.seccomp.json', 'admission.crt', 'admission.key'):
                (infrastructure.output / name).write_text('synthetic fixture')
            infrastructure.command.side_effect = ['x86_64', 'test-kernel', '', '', '', '']
            install_desktop_policy(infrastructure, self.module.IMAGE, 'utility-image')
            webhook = json.loads(infrastructure.k.call_args.kwargs['data'])['webhooks'][0]
        self.assertEqual(webhook['failurePolicy'], 'Fail')
        self.assertEqual(webhook['namespaceSelector'], {
            'matchLabels': {'kubernetes.io/metadata.name': 'cma-native-am'},
        })
        self.assertEqual(webhook['matchConditions'], [{'name': 'desktop-runtime', 'expression':
            'object.spec.containers.exists(container, container.name == "runtime" && '
            f'container.image == {json.dumps(self.module.IMAGE)})'}])


if __name__ == '__main__':
    unittest.main()
