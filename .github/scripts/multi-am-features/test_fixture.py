import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from cleanup import cleanup
from application_fixture import objects as application_objects
from infrastructure import Infrastructure
from run_ci import run as run_ci

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / 'owner'))
from common import CommandFailure
from fixture_no_secrets import NoSecretClient, synthetic_configmaps
import manifests
import legacy_manifests
from run import Fixture
from publication_evidence import FEATURES_MONGO_APP, PREPARED_MONGO_APP
from readiness_scaling import observe_never_ready, observe_scale
from runtime_loss import runtime_inventory_authority


class OwnershipTests(unittest.TestCase):
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
        logs = {'am-a-0': [known, known, entry('owner_authority_refused', digest='a' * 64)],
                'am-b-0': [entry('unknown_commit', 'publish_connection'),
                           entry('work_lease_changed', 'publish_deleted'), entry('unknown_commit', target='unrelated')]}
        fixture = Fixture.__new__(Fixture)

        def read(args, *, timeout):
            self.assertEqual(args[0], 'logs')
            self.assertEqual(args[2:], ['--since=12m', '--limit-bytes=2097152'])
            self.assertEqual(timeout, 8)
            return '\n'.join(logs[args[1]])

        fixture.client = SimpleNamespace(k=read)
        self.assertEqual(fixture.publicationEvidence('fixture-agent'), {
            'a': {'known_aborted_cas': {'open_conversation': 2}},
            'b': {'unknown_commit': {'publish_connection': 1}}})
        with self.assertRaisesRegex(ValueError, 'publication_evidence_lineage_required'):
            fixture.publicationEvidence('../foreign-agent')
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
        self.assertEqual(fixture.retirementPurge('fixture-agent', 'fixture-sandbox', 'fixture-create', True),
                         {'armed': True})
        self.assertIn('"arm": true', script[0])
        for changes in [{'fixture': 'prepared'}, {'context': 'another-kind-context'},
                        {'server': 'https://example.invalid'}, {'run_id': 'foreign'}]:
            fixture.ready = {**local, **changes}
            with self.assertRaisesRegex(ValueError, 'disposable_feature_fixture_required'):
                fixture.retirementPurge('fixture-agent', 'fixture-sandbox', 'fixture-create', True)
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
        self.assertTrue(features_environment['CHRONO_MONGODB_URI'].endswith('&appName=' + FEATURES_MONGO_APP))
        self.assertEqual(features_environment['CHRONO_AGENT_MANAGER_SANDBOX_CPU'], '1')
        self.assertEqual(features_environment['CHRONO_AGENT_MANAGER_SANDBOX_MEMORY'], '8Gi')
        prepared = synthetic_configmaps(manifests.managers(legacy_manifests.credentials(),
                                                            {**ready, 'fixture': 'prepared'}))
        prepared_environment = next(x['data'] for x in prepared if x['kind'] == 'ConfigMap')
        self.assertNotIn('CHRONO_ADMISSION_CONTROL_WRITES_ENABLED', prepared_environment)
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


if __name__ == '__main__':
    unittest.main()
