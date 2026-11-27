"""Offline manifest, native identity and collector orchestration contracts."""
from copy import deepcopy
import json
from pathlib import Path
import secrets
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from migration_kind_manifests import manifests as base_manifests
from migration_membership_fixture import manifests
from migration_membership_documents import infrastructure, job, capture, serving_identity
from migration_fixture_dependencies import configure, capture as selected_capture, runtime_role
from test_migration_membership_validation import snapshot, row, pod


def objects():
    return base_manifests('api', secrets.token_urlsafe(24), secrets.token_urlsafe(24),
                          secrets.token_urlsafe(32), 'mongo', {'createRole': 'cma_am_runtime'})


def ready_document():
    return {'items': [{'metadata': {'uid': 'pod-one', 'name': 'pod-one'}, 'status': {
        'phase': 'Running', 'podIP': '10.0.0.2', 'containerStatuses': [{
            'name': 'api', 'ready': True, 'started': True, 'restartCount': 0,
            'imageID': pod()['image_id'], 'containerID': pod()['container_id'],
            'state': {'running': {}}}]}}]}


class MembershipDocumentsTests(unittest.TestCase):
    def test_manifest_opt_in_exact_downward_fields_and_read_only_roles(self):
        original = objects()
        self.assertIs(configure(original, SimpleNamespace()), original)
        result = manifests(original)
        selected = {(r['kind'], r['metadata']['name']): r for r in result}
        self.assertFalse(any(r['kind'] == 'Secret' for r in result))
        values = selected['ConfigMap', 'api-env']['data']
        self.assertEqual(values['CHRONO_AGENT_MANAGER_REPLICAS_ENABLED'], 'true')
        self.assertEqual(values['CHRONO_AGENT_CONTROL_WRITE_COORDINATION_ENABLED'], 'true')
        spec = selected['Deployment', 'api']['spec']['template']['spec']
        self.assertTrue(spec['automountServiceAccountToken'])
        api, = spec['containers']
        self.assertEqual(api['name'], 'api')
        self.assertEqual(api['startupProbe']['httpGet']['path'], '/livez')
        self.assertEqual([v['valueFrom']['fieldRef']['fieldPath'] for v in api['env'][:-1]],
                         ['status.podIP', 'metadata.name', 'metadata.namespace', 'metadata.uid'])
        self.assertEqual(api['env'][-1]['value'], 'http://$(MATRIX_POD_IP):8080')
        role = selected['Role', 'cma-runtime-sandbox-reclaimer']
        self.assertTrue(all(r['verbs'] == ['get', 'list'] for r in role['rules']))
        script = selected['ConfigMap', 'mongo-init']['data']['init.sh']
        self.assertIn('"collection":"cma_agent_manager_members"},"actions":["find"]', script)
        self.assertIn('"collection":"cma_agent_manager_owners"},"actions":["find"]', script)

    def test_overlay_refuses_ambiguous_configuration_and_passwords(self):
        value = manifests(objects())
        with self.assertRaisesRegex(ValueError, 'unconfigured'):
            manifests(value)
        value = objects()
        api = next(r for r in value if r['kind'] == 'Deployment' and r['metadata']['name'] == 'api')
        api['spec']['template']['spec']['containers'][0]['env'] = [{'name': 'override'}]
        with self.assertRaisesRegex(ValueError, 'without overrides'):
            manifests(value)
        for password in [None, '', 'x' * 65, 'quote\'unsafe']:
            with self.assertRaises(ValueError):
                infrastructure(objects(), 'cma-migration', password)

    def test_binary_supplies_enabled_role_and_default_call_is_unchanged(self):
        for enabled in (False, True):
            command = Mock(return_value='{"createRole":"cma_am_runtime"}')
            self.assertEqual(runtime_role(command, SimpleNamespace(old_image='pinned', membership_fixture=enabled)),
                             {'createRole': 'cma_am_runtime'})
            argv = command.call_args.args[0]
            self.assertEqual(argv[-2:], ['pinned', '--print-runtime-role'])
            self.assertEqual('CHRONO_AGENT_MANAGER_REPLICAS_ENABLED=true' in argv, enabled)
            self.assertEqual(argv[:5], ['docker', 'run', '--rm', '--network', 'none'])
        with self.assertRaises(json.JSONDecodeError):
            runtime_role(Mock(return_value='broken'), SimpleNamespace(old_image='pinned'))

    def test_job_is_finite_read_only_and_not_a_pod_exec(self):
        value = job('cma-migration', 'mongo', 'old-write')
        spec = value['spec']['template']['spec']
        self.assertEqual(value['spec']['backoffLimit'], 0)
        self.assertEqual(value['spec']['activeDeadlineSeconds'], 120)
        self.assertFalse(spec['automountServiceAccountToken'])
        self.assertEqual(spec['containers'][0]['command'],
                         ['mongosh', '--nodb', '--quiet', '/catalog/read.cjs'])
        self.assertEqual(spec['volumes'][0]['configMap']['name'], 'mongo-membership-code')
        with self.assertRaises(RuntimeError):
            job('namespace', 'mongo', 'unknown')

    def campaign(self, directory, source='source', image='image'):
        old = {'source': source, 'image': image}
        new = {'source': 'new', 'image': 'new-image'}
        readers = {'historical': old, 'current': new, 'candidate': new, 'recovery': old}
        return SimpleNamespace(output=Path(directory), active_reader={'pod': pod(), 'source': source},
                               reader_artifacts=readers,
                               reader_pods=Mock(return_value=ready_document()),
                               kubectl=Mock(side_effect=['', '', json.dumps(snapshot())]))

    def test_exact_running_container_and_pod_required(self):
        with tempfile.TemporaryDirectory() as directory:
            campaign = self.campaign(directory)
            self.assertEqual(serving_identity(campaign), pod())
            for transform in [lambda d: d['items'].clear(),
                              lambda d: d['items'].append(deepcopy(d['items'][0])),
                              lambda d: d['items'][0]['metadata'].update(deletionTimestamp='now'),
                              lambda d: d['items'][0]['status'].update(containerStatuses=[]),
                              lambda d: d['items'][0]['status']['containerStatuses'][0].update(started=False),
                              lambda d: d['items'][0]['status']['containerStatuses'][0].update(restartCount=1)]:
                value = ready_document(); transform(value)
                campaign.reader_pods.return_value = value
                with self.assertRaises(ValueError):
                    serving_identity(campaign)

    def test_capture_records_observation_without_claiming_reader_compatibility(self):
        with tempfile.TemporaryDirectory() as directory:
            campaign = self.campaign(directory)
            result = capture(campaign, 'cma-migration', 'mongo', 'old-write', 'source', 'image')
            self.assertEqual(result['outcome'], 'OBSERVED')
            self.assertEqual(result['application_cross_reader_compatibility'], 'NOT_PROVEN')
            self.assertEqual(result['comparisons'], [])
            raw = campaign.output / result['path']
            self.assertEqual(raw.stat().st_mode & 0o777, 0o600)
            self.assertEqual(len(campaign.kubectl.call_args_list), 3)
            with patch('migration_membership_documents.capture', return_value=result) as read:
                self.assertEqual(selected_capture(campaign, SimpleNamespace(membership_fixture=True),
                                                  'namespace', 'mongo', 'stage', 'source', 'image'), result)
                read.assert_called_once()

    def test_capture_compares_prior_hashes_and_exact_reader(self):
        with tempfile.TemporaryDirectory() as directory:
            campaign = self.campaign(directory, 'old', 'old-image')
            first = capture(campaign, 'n', 'mongo', 'old-write', 'old', 'old-image')
            second_pod = pod('second', '10.0.0.3')
            doc = ready_document(); doc['items'][0]['metadata'].update(uid='second', name='second')
            doc['items'][0]['status']['podIP'] = '10.0.0.3'
            campaign.active_reader['pod'] = second_pod
            campaign.active_reader['source'] = 'new'
            campaign.reader_pods.return_value = doc
            raw = json.dumps(snapshot([row('new', '10.0.0.3', 40000)], 25000))
            campaign.kubectl.side_effect = ['', '', raw]
            second = capture(campaign, 'n', 'mongo', 'new-read-old-write', 'new', 'new-image')
            self.assertTrue(second['comparisons'][0]['reader_replaced'])
            (campaign.output / first['path']).write_text('{}')
            campaign._membership_history.pop()
            campaign.kubectl.side_effect = ['', '', raw]
            with self.assertRaisesRegex(ValueError, 'hash differs'):
                capture(campaign, 'n', 'mongo', 'new-read-old-write', 'new', 'new-image')

    def test_capture_preserves_failure_and_raw_diagnostics(self):
        with tempfile.TemporaryDirectory() as directory:
            campaign = self.campaign(directory, 's', 'i')
            campaign.kubectl.side_effect = RuntimeError('collector job failed')
            with self.assertRaisesRegex(RuntimeError, 'job failed'):
                capture(campaign, 'n', 'mongo', 'old-write', 's', 'i')
            for raw in ['malformed', 'x' * 65537, json.dumps(snapshot([]))]:
                campaign.kubectl.side_effect = ['', '', raw]
                with self.assertRaises(ValueError):
                    capture(campaign, 'n', 'mongo', 'old-write', 's', 'i')
                self.assertEqual((campaign.output / 'membership-old-write.private.json').read_text(), raw)
            campaign.kubectl.side_effect = ['', '', json.dumps(snapshot())]
            changed = ready_document(); changed['items'][0]['status']['podIP'] = '10.0.0.3'
            campaign.reader_pods.side_effect = [ready_document(), changed]
            with self.assertRaisesRegex(ValueError, 'reader changed'):
                capture(campaign, 'n', 'mongo', 'old-write', 's', 'i')


if __name__ == '__main__':
    unittest.main()
