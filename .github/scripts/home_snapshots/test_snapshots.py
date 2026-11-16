"""Offline contracts; never calls Kubernetes, Compute, or a customer runtime."""
import copy
import contextlib
import io
import unittest
from unittest.mock import patch
from urllib.parse import urlsplit

from policy import (ANNOTATION, CLASS, COHORT, DRIVER, LABEL, binding, disk_handle,
                    policy_body, policy_ref, request_id, validate_disk, validate_policy)
from reconcile import reconcile_one, run
from render import render

CONFIG = {'project': 'fixture-project', 'region': 'asia-southeast1', 'namespace': 'fixture-homes',
          'policy': 'home-hourly-v1', 'activation_time': '2026-11-17T00:00:00Z',
          'kms_key': 'projects/fixture-project/locations/asia-southeast1/keyRings/homes/cryptoKeys/key'}
IMAGE = 'fixture/python@sha256:' + 'a' * 64
GSA = 'snapshot-attach@fixture-project.iam.gserviceaccount.com'


def fixture(regional=False):
    source = {'apiVersion': 'storage.k8s.io/v1', 'kind': 'StorageClass',
              'metadata': {'name': 'home-a', 'uid': 'old-class', 'creationTimestamp': '2020-01-01T00:00:00Z'},
              'provisioner': DRIVER, 'parameters': {'type': 'hyperdisk-balanced', 'disk-encryption-kms-key': CONFIG['kms_key']},
              'allowVolumeExpansion': True, 'reclaimPolicy': 'Delete', 'volumeBindingMode': 'WaitForFirstConsumer'}
    sc = render(source, CONFIG, IMAGE, GSA)['resources.json']['items'][0]
    sc['metadata'].update(uid='new-class', creationTimestamp='2026-11-17T00:00:01Z')
    pvc = {'metadata': {'uid': '00000000-0000-4000-8000-000000000001', 'name': 'home-fixture',
                       'namespace': CONFIG['namespace'], 'creationTimestamp': '2026-11-17T00:00:02Z',
                       'annotations': {'cma/home-layout': 'native-vct-full-home-v2'}},
           'spec': {'storageClassName': CLASS, 'accessModes': ['ReadWriteOncePod']}, 'status': {'phase': 'Bound'}}
    name = 'pvc-' + pvc['metadata']['uid']
    pvc['spec']['volumeName'] = name
    scope = 'regions/asia-southeast1' if regional else 'zones/asia-southeast1-b'
    handle = f"projects/{CONFIG['project']}/{scope}/disks/{name}"
    pv = {'metadata': {'uid': 'pv-uid', 'name': name, 'creationTimestamp': '2026-11-17T00:00:04Z',
                      'annotations': {'pv.kubernetes.io/provisioned-by': DRIVER}},
          'spec': {'storageClassName': CLASS, 'claimRef': {key: pvc['metadata'][key] for key in ('name', 'namespace', 'uid')},
                   'csi': {'driver': DRIVER, 'volumeHandle': handle}}, 'status': {'phase': 'Bound'}}
    disk = {'selfLink': 'https://www.googleapis.com/compute/v1/' + handle, 'id': '123456',
            'name': name, 'status': 'READY', 'creationTimestamp': '2026-11-17T00:00:03Z',
            'labels': {LABEL: COHORT}, 'diskEncryptionKey': {'kmsKeyName': CONFIG['kms_key']},
            'type': 'hyperdisk-balanced', 'resourcePolicies': []}
    return source, sc, pvc, pv, disk


class FakeClient:
    def __init__(self, regional=False):
        self.source, self.sc, self.pvc, self.pv, self.disk = fixture(regional)
        self.reads, self.writes, self.pages = [], [], None
        self.operation = {'name': 'attach-1', 'status': 'DONE'}
        self.policy = {**policy_body(CONFIG), 'selfLink': policy_ref(CONFIG), 'status': 'READY'}

    def kget(self, path):
        self.reads.append(path)
        if 'storageclasses' in path:
            return copy.deepcopy(self.sc)
        if '?' in path:
            return copy.deepcopy(self.pages.pop(0) if self.pages else {'items': [self.pvc]})
        return copy.deepcopy(self.pv if '/persistentvolumes/' in path else self.pvc)

    def compute(self, path, body=None):
        self.reads.append(path)
        if body is not None:
            self.writes.append((path, copy.deepcopy(body)))
            self.disk['resourcePolicies'] = body['resourcePolicies']
            return copy.deepcopy(self.operation)
        if '/operations/' in path:
            return {'name': 'attach-1', 'status': 'DONE'}
        if '/resourcePolicies/' in path:
            return copy.deepcopy(self.policy)
        return copy.deepcopy(self.disk)


class Selection(unittest.TestCase):
    def test_zonal_and_regional_identity_and_attach_readback(self):
        for regional in (False, True):
            with self.subTest(regional=regional):
                client = FakeClient(regional)
                self.assertEqual(reconcile_one(client, client.pvc, client.sc, CONFIG, True), 'attached')
                self.assertEqual(len(client.writes), 1)
                self.assertIn('/addResourcePolicies?requestId=', client.writes[0][0])
                self.assertEqual(client.writes[0][1], {'resourcePolicies': [policy_ref(CONFIG)]})
                self.assertEqual(reconcile_one(client, client.pvc, client.sc, CONFIG, True), 'attached')
                self.assertEqual(len(client.writes), 1)

    def test_observe_default_has_zero_writes(self):
        client = FakeClient()
        self.assertEqual(reconcile_one(client, client.pvc, client.sc, CONFIG), 'would_attach')
        self.assertEqual(client.writes, [])

    def test_existing_home_a_and_unrelated_claims_are_not_even_read(self):
        client = FakeClient()
        client.pvc['spec']['storageClassName'] = 'home-a'
        with contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(run(client, CONFIG, True), 0)
        self.assertEqual(client.writes, [])
        self.assertNotIn('/persistentvolumes/', '\n'.join(client.reads))
        self.assertIn('"selected": 0', output.getvalue())

    def test_all_identity_and_cohort_refusals_prevent_writes(self):
        edits = [
            ('sc', 'metadata/creationTimestamp', '2025-01-01T00:00:00Z'),
            ('pvc', 'metadata/creationTimestamp', '2025-01-01T00:00:00Z'),
            ('pvc', 'metadata/namespace', 'another-namespace'),
            ('pvc', 'metadata/deletionTimestamp', '2026-11-17T00:00:05Z'),
            ('pvc', 'status/phase', 'Pending'), ('pvc', 'spec/dataSource', {'kind': 'VolumeSnapshot'}),
            ('pvc', 'spec/accessModes', ['ReadWriteOnce']),
            ('pv', 'spec/claimRef/uid', 'another-claim'), ('pv', 'spec/csi/driver', 'other-driver'),
            ('pv', 'spec/storageClassName', 'home-a'), ('pv', 'metadata/deletionTimestamp', 'now'),
            ('disk', 'labels/cma-home-backup', 'old'), ('disk', 'id', ''),
            ('disk', 'creationTimestamp', '2025-01-01T00:00:00Z'),
            ('disk', 'diskEncryptionKey/kmsKeyName', 'another-key'),
            ('disk', 'sourceSnapshot', 'existing-snapshot'), ('disk', 'type', 'pd-standard'),
            ('disk', 'resourcePolicies', ['another-policy']),
        ]
        for target, path, value in edits:
            with self.subTest(target=target, path=path):
                client = FakeClient()
                obj = getattr(client, target)
                parts = path.split('/')
                for part in parts[:-1]:
                    obj = obj[part]
                obj[parts[-1]] = value
                with self.assertRaises((ValueError, KeyError)):
                    reconcile_one(client, client.pvc, client.sc, CONFIG, True)
                self.assertEqual(client.writes, [])

    def test_cross_project_region_and_malformed_handles(self):
        for handle in ['projects/foreign/zones/asia-southeast1-b/disks/a',
                       'projects/fixture-project/zones/us-east1-b/disks/a',
                       'https://evil.invalid/disks/a', 'projects/fixture-project/zones/../disks/a']:
            with self.subTest(handle=handle), self.assertRaises(ValueError):
                disk_handle(handle, CONFIG['project'], CONFIG['region'])

    def test_policy_mutations_refuse_all_work(self):
        for field, value in [('schedule', {'hourlySchedule': {'hoursInCycle': 2, 'startTime': '00:00'}}),
                             ('retentionPolicy', {'maxRetentionDays': 7, 'onSourceDiskDelete': 'APPLY_RETENTION_POLICY'})]:
            client = FakeClient()
            client.policy['snapshotSchedulePolicy'][field] = value
            with self.assertRaises(ValueError):
                run(client, CONFIG, True)
            self.assertEqual(client.writes, [])

    def test_racing_pvc_replacement_stops_before_attach(self):
        client = FakeClient()
        original = client.kget
        count = 0
        def get(path):
            nonlocal count
            value = original(path)
            if '/persistentvolumeclaims/' in path:
                count += 1
                if count == 2:
                    value['metadata']['uid'] = 'replacement'
            return value
        client.kget = get
        with self.assertRaisesRegex(ValueError, 'pvc_replaced'):
            reconcile_one(client, client.pvc, client.sc, CONFIG, True)
        self.assertEqual(client.writes, [])

    def test_timeout_then_retry_keeps_original_operation_identity(self):
        client = FakeClient()
        original = client.compute
        def uncertain(path, body=None):
            if body is not None:
                client.writes.append((path, body))
                raise TimeoutError('private upstream body')
            return original(path)
        client.compute = uncertain
        for _ in range(2):
            with contextlib.redirect_stdout(io.StringIO()) as output:
                self.assertEqual(run(client, CONFIG, True), 1)
            self.assertNotIn('private upstream body', output.getvalue())
        self.assertEqual(client.writes[0], client.writes[1])
        old = request_id(client.disk, CONFIG)
        client.disk['id'] = '999'
        self.assertNotEqual(old, request_id(client.disk, CONFIG))

    def test_operation_polls_correct_scope_and_refuses_error(self):
        for regional in (False, True):
            client = FakeClient(regional)
            client.operation['status'] = 'PENDING'
            with patch('reconcile.time.sleep'):
                self.assertEqual(reconcile_one(client, client.pvc, client.sc, CONFIG, True), 'attached')
            scope = 'regions/asia-southeast1' if regional else 'zones/asia-southeast1-b'
            self.assertIn(f"projects/{CONFIG['project']}/{scope}/operations/attach-1", client.reads)
        client = FakeClient()
        client.operation['error'] = {'errors': [{'code': 'QUOTA_EXCEEDED'}]}
        with self.assertRaisesRegex(ValueError, 'attach_operation_failed'):
            reconcile_one(client, client.pvc, client.sc, CONFIG, True)

    def test_pages_are_joined_before_any_attach_and_errors_are_not_coverage(self):
        client = FakeClient()
        client.pages = [{'items': [client.pvc], 'metadata': {'continue': 'more'}},
                        {'items': [], 'metadata': {}}]
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(run(client, CONFIG, True), 0)
        self.assertEqual(len(client.writes), 1)
        client = FakeClient()
        client.pages = [{'items': [client.pvc], 'metadata': {'continue': 'same'}}] * 2
        with self.assertRaisesRegex(ValueError, 'pagination_loop'):
            run(client, CONFIG, True)
        self.assertEqual(client.writes, [])


class Packet(unittest.TestCase):
    def test_render_is_suspended_readonly_and_immutable_with_minimum_rbac(self):
        source = fixture()[0]
        before = copy.deepcopy(source)
        packet = render(source, CONFIG, IMAGE, GSA)
        self.assertEqual(source, before)
        items = packet['resources.json']['items']
        sc = items[0]
        self.assertNotIn('uid', sc['metadata'])
        self.assertEqual(sc['parameters']['disk-encryption-kms-key'], CONFIG['kms_key'])
        self.assertNotIn('resource-policies', sc['parameters'])
        cron = items[-1]['spec']
        self.assertTrue(cron['suspend'])
        self.assertEqual(cron['concurrencyPolicy'], 'Forbid')
        pod = cron['jobTemplate']['spec']['template']['spec']
        self.assertNotIn('--apply', pod['containers'][0]['args'])
        self.assertTrue(items[-2]['immutable'])
        for item in items:
            for rule in item.get('rules', []):
                self.assertTrue(set(rule['verbs']) <= {'get', 'list'})
                self.assertNotIn('secrets', rule['resources'])
        permissions = packet['iam-role.json']['includedPermissions']
        self.assertEqual([p for p in permissions if not p.endswith(('.get', '.use'))], ['compute.disks.addResourcePolicies'])
        self.assertEqual(packet['new-home-default.patch.json'][0]['value'], 'home-a')
        self.assertEqual(packet['rollback-default.patch.json'][0]['value'], CLASS)
        self.assertTrue(packet['rollback-job.patch.json'][0]['value'])

    def test_bad_image_and_changed_source_are_rejected(self):
        for image in ['python:latest', 'python:3.12']:
            with self.assertRaisesRegex(ValueError, 'immutable_image_required'):
                render(fixture()[0], CONFIG, image, GSA)
        source = fixture()[0]
        source['parameters']['labels'] = 'unrelated=keep'
        with self.assertRaisesRegex(ValueError, 'source_labels_require_review'):
            render(source, CONFIG, IMAGE, GSA)


if __name__ == '__main__':
    unittest.main()
