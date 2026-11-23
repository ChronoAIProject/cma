import json
from pathlib import Path
import secrets
import tempfile
import unittest
from unittest.mock import Mock, patch
from urllib.parse import parse_qs, urlsplit

from migration_boot_observation import BootObservationError, evaluate, observe_boot
from migration_kind_pods import select_ready_api_pod
import migration_kind_runner as runner


IMAGE = 'candidate-api'
CONFIG = 'sha256:' + 'a' * 64
SELECTED = {'uid': 'healthy', 'name': 'api-healthy'}


def pod(uid='healthy', ready=False, image=IMAGE):
    return {'metadata': {'uid': uid, 'name': 'api-' + uid,
                         'resourceVersion': 'opaque-' + uid + '-' + str(ready)},
            'spec': {'containers': [{'name': 'api', 'image': image}]},
            'status': {'phase': 'Running' if ready else 'Pending',
                       'conditions': [{'type': 'Ready', 'status': str(ready)}],
                       'containerStatuses': [{'name': 'api', 'image': image,
                                              'imageID': 'containerd://' + CONFIG,
                                              'restartCount': 0, 'ready': ready,
                                              'state': {'running': {}} if ready else
                                                       {'waiting': {'reason': 'ContainerCreating'}}}]}}


def stream(*events):
    return '\n'.join(json.dumps({'type': kind, 'object': value}) for kind, value in events)


def success():
    return stream(('ADDED', pod()), ('MODIFIED', pod(ready=True)))


class BootObservationTests(unittest.TestCase):
    def test_exact_manifest_is_allowed_and_embedded_config_is_rejected(self):
        image = 'example.invalid/api@sha256:' + 'b'*64
        for image_id in (image, image.split('@')[1], 'docker-pullable://'+image, 'containerd://'+CONFIG):
            candidate = pod(ready=True, image=image)
            candidate['status']['containerStatuses'][0]['imageID'] = image_id
            result = evaluate(stream(('ADDED', candidate)), set(), SELECTED, image, CONFIG, 'before')
            self.assertEqual(result['failed_boots'], 0)
        for image_id in ('prefix'+CONFIG, CONFIG+'suffix', 'unknown://'+CONFIG):
            candidate = pod(ready=True, image=image)
            candidate['status']['containerStatuses'][0]['imageID'] = image_id
            with self.assertRaises(BootObservationError):
                evaluate(stream(('ADDED', candidate)), set(), SELECTED, image, CONFIG, 'before')

    def test_replays_from_opaque_list_version_and_keeps_expected_retirement(self):
        old = pod('old', ready=True, image='historical-api')
        old['status']['phase'] = 'Failed'
        before = {'metadata': {'resourceVersion': 'opaque/start+version'}, 'items': [old]}
        command = Mock(return_value=stream(('DELETED', old)) + '\n' + success())
        result = observe_boot(command, 'cma-migration', before, SELECTED, IMAGE, CONFIG)
        self.assertEqual(result['failed_boots'], 0)
        self.assertEqual(result['attempted_uids'], ['healthy'])
        self.assertEqual(result['ready_uids'], ['healthy'])
        args, kwargs = command.call_args
        query = parse_qs(urlsplit(args[0][-1]).query)
        self.assertEqual(query['resourceVersion'], ['opaque/start+version'])
        self.assertEqual(query['labelSelector'], ['app=api'])
        self.assertEqual(query['timeoutSeconds'], ['1'])
        self.assertEqual(kwargs['timeout'], 20)

    def test_deleted_failed_attempt_cannot_be_hidden_by_healthy_replacement(self):
        failed = pod('failed')
        failed['status']['phase'] = 'Failed'
        events = stream(('ADDED', pod('failed')), ('MODIFIED', failed), ('DELETED', failed))
        # The previous final-snapshot check accepts the replacement by itself.
        selected = select_ready_api_pod({'items': [pod(ready=True)]}, IMAGE, CONFIG)
        self.assertEqual(selected['uid'], 'healthy')
        with self.assertRaisesRegex(BootObservationError, 'failed API startup'):
            evaluate(events + '\n' + success(), set(), selected, IMAGE, CONFIG, 'before')

    def test_replacement_before_ready_is_a_failed_boot_even_without_exit_code(self):
        raw = stream(('ADDED', pod('lost')), ('DELETED', pod('lost'))) + '\n' + success()
        with self.assertRaisesRegex(BootObservationError, 'replaced before becoming Ready'):
            evaluate(raw, set(), SELECTED, IMAGE, CONFIG, 'before')

    def test_restart_termination_and_container_start_errors_are_rejected(self):
        mutations = [
            lambda c: c.update(restartCount=1),
            lambda c: c.update(restartCount=False),
            lambda c: c.pop('restartCount'),
            lambda c: c.update(lastState={'terminated': {'exitCode': 1}}),
            lambda c: c.update(state={'terminated': {'exitCode': 0}}),
            lambda c: c.update(state={'waiting': {'reason': 'CrashLoopBackOff'}}),
            lambda c: c.update(state={'waiting': {'reason': 'ImagePullBackOff'}}),
        ]
        for mutate in mutations:
            with self.subTest(mutate=mutate):
                changed = pod(ready=True)
                mutate(changed['status']['containerStatuses'][0])
                with self.assertRaisesRegex(BootObservationError, 'failed API startup'):
                    evaluate(stream(('ADDED', pod()), ('MODIFIED', changed)),
                             set(), SELECTED, IMAGE, CONFIG, 'before')

    def test_expired_missing_truncated_or_unknown_history_never_passes(self):
        examples = [
            '', '{"type":', '{}', '[]',
            stream(('ERROR', {'code': 410, 'reason': 'Expired'})),
            stream(('BOOKMARK', {'metadata': {}})),
            stream(('UNKNOWN', pod())),
            stream(('MODIFIED', pod(ready=True))),
            stream(('ADDED', pod())),
            stream(('BOOKMARK', {'metadata': {'resourceVersion': 'new'}})),
            '\n'.join(['{}'] * 2049),
        ]
        for raw in examples:
            with self.subTest(raw=raw[:80]):
                with self.assertRaises(BootObservationError):
                    evaluate(raw, set(), SELECTED, IMAGE, CONFIG, 'before')

    def test_transport_failure_is_propagated_without_rewatch_or_success(self):
        command = Mock(side_effect=RuntimeError('watch disconnected'))
        with self.assertRaisesRegex(RuntimeError, 'watch disconnected'):
            observe_boot(command, 'cma-migration',
                         {'metadata': {'resourceVersion': 'before'}, 'items': []},
                         SELECTED, IMAGE, CONFIG)
        command.assert_called_once()

    def test_invalid_baseline_or_existing_selected_pod_never_claims_a_new_boot(self):
        command = Mock()
        for before in ({'items': []}, {'metadata': {'resourceVersion': 'before'}},
                       {'metadata': {'resourceVersion': 'before'}, 'items': [pod()]}):
            with self.subTest(before=before):
                with self.assertRaises(BootObservationError):
                    observe_boot(command, 'cma-migration', before, SELECTED, IMAGE, CONFIG)
        command.assert_not_called()

    def test_image_config_and_pod_identity_are_required(self):
        wrong_image = pod(image='unexpected-api')
        wrong_config = pod(ready=True)
        wrong_config['status']['containerStatuses'][0]['imageID'] = 'containerd://sha256:' + 'b' * 64
        missing_uid = pod()
        missing_uid['metadata'].pop('uid')
        for candidate in (wrong_image, wrong_config, missing_uid):
            with self.subTest(candidate=candidate):
                with self.assertRaises(BootObservationError):
                    evaluate(stream(('ADDED', candidate)) + '\n' + success(),
                             set(), SELECTED, IMAGE, CONFIG, 'before')

    def test_retained_observations_exclude_environment_and_annotations(self):
        private = secrets.token_urlsafe(24)
        candidate = pod(ready=True)
        candidate['metadata']['annotations'] = {'synthetic-private': private}
        candidate['spec']['containers'][0]['env'] = [{'name': 'PASSWORD', 'value': private}]
        result = evaluate(stream(('ADDED', pod()), ('MODIFIED', candidate)),
                          set(), SELECTED, IMAGE, CONFIG, 'before')
        self.assertNotIn(private, json.dumps(result))
        self.assertEqual(result['observations'][-1]['uid'], SELECTED['uid'])


class BootRunnerTests(unittest.TestCase):
    def test_reader_snapshot_preserves_server_resource_version(self):
        with tempfile.TemporaryDirectory() as directory:
            plan = Path(directory) / 'plan.json'
            plan.write_text('{}')
            campaign = runner.Campaign(directory, 'kind-not-invoked', 1, plan)
            server = {'apiVersion': 'v1', 'kind': 'PodList',
                      'metadata': {'resourceVersion': 'opaque-before-create'}, 'items': []}
            # kubectl's client-generated List drops the server's list version.
            # This matches the saved response from the first RAM-backed run.
            client = {**server, 'kind': 'List', 'metadata': {'resourceVersion': ''}}

            def kubectl(args, **kwargs):
                if '--raw' not in args:
                    return json.dumps(client)
                target = urlsplit(args[args.index('--raw') + 1])
                self.assertEqual(target.path, '/api/v1/namespaces/cma-migration/pods')
                self.assertEqual(parse_qs(target.query), {'labelSelector': ['app=api']})
                self.assertEqual(kwargs['timeout'], 20)
                return json.dumps(server)

            with patch.object(campaign, 'kubectl', side_effect=kubectl) as command:
                before = campaign.reader_pods()
            self.assertEqual(before, server)
            command.assert_called_once()
            receipt = observe_boot(Mock(return_value=success()), 'cma-migration',
                                   before, SELECTED, IMAGE, CONFIG)
            self.assertEqual(receipt['start_resource_version'], 'opaque-before-create')

    def test_create_captures_history_before_the_first_api_apply(self):
        with tempfile.TemporaryDirectory() as directory:
            plan = Path(directory) / 'plan.json'
            plan.write_text('{}')
            campaign = runner.Campaign(directory, 'kind-not-invoked', 1, plan)
            campaign.images = []
            before = {'metadata': {'resourceVersion': 'before-apply'}, 'items': []}
            applied = []

            def command(argv, input_text=None, **kwargs):
                if 'apply' in argv:
                    value = json.loads(input_text)
                    if value['kind'] == 'Deployment':
                        applied.append(value)
                return ''

            def snapshot():
                self.assertEqual(applied, [])
                return before

            with patch.object(campaign, 'command', side_effect=command), \
                    patch.object(campaign, 'reader_pods', side_effect=snapshot) as reader:
                campaign.create([{'kind': 'Deployment', 'metadata': {'name': 'api'}}])
            reader.assert_called_once()
            self.assertEqual(len(applied), 1)
            self.assertEqual(campaign.pending_boot['pods'], before)

    def test_initial_boot_uses_snapshot_captured_before_api_was_applied(self):
        with tempfile.TemporaryDirectory() as directory:
            plan = Path(directory) / 'plan.json'
            plan.write_text('{}')
            campaign = runner.Campaign(directory, 'kind-not-invoked', 1, plan)
            campaign.pending_boot = {'pods': {'metadata': {'resourceVersion': 'before-apply'},
                                              'items': []}, 'started_at': 123}
            ready = {'items': [pod(ready=True)]}
            calls = []

            def kubectl(args, **kwargs):
                calls.append(args)
                return success() if '--raw' in args else ''

            with patch.object(campaign, 'reader_pods', return_value=ready) as reader, \
                    patch.object(campaign, 'kubectl', side_effect=kubectl):
                receipt = campaign.boot(IMAGE, 'a' * 40, 'historical-boot', CONFIG)
            reader.assert_called_once()
            self.assertEqual(receipt['started_at'], 123)
            self.assertEqual(receipt['startup_observation']['start_resource_version'], 'before-apply')
            self.assertIsNone(campaign.pending_boot)
            self.assertTrue(any('--raw' in call for call in calls))


if __name__ == '__main__':
    unittest.main()
