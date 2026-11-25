"""Actual row presence, immutable contents and quota effects are mandatory."""
from copy import deepcopy
import json
from pathlib import Path
import secrets
import tempfile
import unittest

import migration_launch_documents as documents
from migration_kind_manifests import manifests
from migration_browser_fixture import OWNER


def number(value):
    return {'$numberLong': str(value)}


def snapshot(count=2):
    launches, protocols, records = [], [], []
    for offset in range(0, count, 2):
        saved = {'writer': 'old-write' if offset == 0 else 'new-write', 'day': 20000, 'launches': []}
        for index in range(offset, offset + 2):
            key = 'launch-' + str(index)
            protocol = {'schema_version': number(1), 'launch_id': key,
                        'command_fingerprint': 'a' * 64, 'created_at': '2024-10-04T00:00:00Z',
                        'initiation_sha256': 'b' * 64}
            row = {'_id': key, 'owner': OWNER, 'public_id': 'public-' + str(offset),
                   'document_schema_version': number(1), 'command_key_hash': str(index)}
            protocols.append({**deepcopy(row), 'record': deepcopy(protocol)})
            launches.append({**row, 'record': {'id': key, 'authority': 'visitor', 'state': 'draft',
                'created_at': protocol['created_at'], 'command_fingerprint': protocol['command_fingerprint']},
                'trigger_input_protocol': protocol})
            saved['launches'].append({'snapshot': {'detail': {'id': key}}})
        records.append(saved)
    value = {'schema': documents.SCHEMA, 'database': 'matrix', 'collections': {
        'trigger_launches': launches, 'trigger_launch_input_protocols': protocols,
        'visitor_launch_days': [{'_id': '20000:' + OWNER, 'document_schema_version': number(1),
            'used': number(count), 'expires_at': {'$date': number(20002 * 86400000)}}]}}
    return value, records


class DocumentTests(unittest.TestCase):
    def test_both_writer_directions_require_actual_nonempty_bound_receipts(self):
        old, old_records = snapshot()
        new, new_records = snapshot(4)
        documents.validate(old, old_records, 2)
        documents.validate(new, new_records, 4)
        documents.compare(old['collections'], deepcopy(old['collections']), False)
        documents.compare(old['collections'], new['collections'], True)
        documents.compare(new['collections'], deepcopy(new['collections']), False)

    def test_missing_duplicate_foreign_and_extra_documents_fail(self):
        for name in documents.COLLECTIONS:
            for fault in ('empty', 'duplicate', 'foreign'):
                value, records = snapshot()
                rows = value['collections'][name]
                if fault == 'empty': rows.clear()
                elif fault == 'duplicate': rows.append(deepcopy(rows[0]))
                else: rows[0]['_id'] = 'foreign'
                with self.subTest(name=name, fault=fault), self.assertRaises(RuntimeError):
                    documents.validate(value, records, 2)
        value, records = snapshot(); value['collections']['auth_subjects'] = []
        with self.assertRaisesRegex(RuntimeError, 'scope'):
            documents.validate(value, records, 2)

    def test_protocol_and_quota_mismatch_fail(self):
        faults = [lambda c: c['trigger_launch_input_protocols'][0]['record'].update(initiation_sha256='0' * 64),
                  lambda c: c['trigger_launches'][0].pop('trigger_input_protocol'),
                  lambda c: c['trigger_launch_input_protocols'][0].update(owner='foreign'),
                  lambda c: c['trigger_launches'][0]['record'].update(authority='sandbox_creator'),
                  lambda c: c['visitor_launch_days'][0].update(used=number(0)),
                  lambda c: c['visitor_launch_days'][0].update(expires_at={'$date': number(0)}),
                  lambda c: c['trigger_launches'][0].update(document_schema_version=number(2))]
        for fault in faults:
            value, records = snapshot(); fault(value['collections'])
            with self.subTest(fault=fault), self.assertRaises(RuntimeError):
                documents.validate(value, records, 2)

    def test_reader_rewrite_writer_rewrite_and_quota_reset_fail(self):
        old, _ = snapshot(); new, _ = snapshot(4)
        for name in documents.COLLECTIONS:
            after = deepcopy(old['collections']); after[name][0]['extra'] = True
            with self.assertRaisesRegex(RuntimeError, 'reader changed'):
                documents.compare(old['collections'], after, False)
            after = deepcopy(new['collections']); after[name][0]['extra'] = True
            with self.assertRaises(RuntimeError):
                documents.compare(old['collections'], after, True)
        new['collections']['visitor_launch_days'][0]['used'] = number(2)
        with self.assertRaisesRegex(RuntimeError, 'quota beyond'):
            documents.compare(old['collections'], new['collections'], True)

    def test_canonical_bson_numbers_and_snapshot_identity_are_strict(self):
        for value in (1, True, {}, {'$numberDouble': '1'}, {'$numberLong': '-1'},
                      {'$numberInt': '1', '$numberLong': '1'}, {'$numberLong': '01'}):
            with self.subTest(value=value), self.assertRaises(RuntimeError): documents.integer(value)
        self.assertEqual(documents.integer({'$numberInt': '1'}), 1)
        value, records = snapshot(); value['database'] = 'admin'
        with self.assertRaises(RuntimeError): documents.validate(value, records, 2)
        value, records = snapshot(4); records[1]['day'] += 1
        with self.assertRaisesRegex(RuntimeError, 'UTC days'): documents.validate(value, records, 4)

    def test_scoped_find_role_no_secret_exec_or_document_mutation(self):
        password = secrets.token_urlsafe(24)
        objects = manifests('api', 'runtime', 'admin', 'key', 'mongo', {})
        documents.infrastructure(objects, 'cma-migration', password)
        self.assertFalse(any(obj['kind'] == 'Secret' for obj in objects))
        init = next(o for o in objects if o['kind'] == 'ConfigMap' and o['metadata']['name'] == 'mongo-init')
        role = init['data']['init.sh'].split('role:"matrix_launch_reader",privileges:')[1].split(',roles:[]')[0]
        permissions = json.loads(role)
        self.assertEqual({p['resource']['collection'] for p in permissions}, set(documents.COLLECTIONS))
        self.assertTrue(all(p['actions'] == ['find'] and p['resource']['db'] == 'matrix' for p in permissions))
        job = documents.job('cma-migration', 'pinned-mongo', 'old-write')
        self.assertEqual(job['spec']['backoffLimit'], 0)
        self.assertFalse(job['spec']['template']['spec']['automountServiceAccountToken'])
        self.assertNotIn(password, json.dumps(job))
        with self.assertRaises(ValueError): documents.infrastructure([], 'cma-migration', "bad'password")

    def test_capture_binds_actual_records_and_rejects_failed_reader_or_job(self):
        with tempfile.TemporaryDirectory() as temp:
            value, records = snapshot()
            class Campaign:
                output = Path(temp)
                fail = False
                def kubectl(self, args, **kwargs):
                    if self.fail: raise RuntimeError('job failed')
                    assert 'exec' not in args
                    return json.dumps(value) if 'logs' in args else ''
            campaign = Campaign()
            (campaign.output / 'old-write-visitor.private.json').write_text(json.dumps(records[0]))
            for stage in ('old-write', 'new-read-old-write'):
                result = documents.capture(campaign, 'cma-migration', 'mongo', stage, 'a' * 40, 'api')
                self.assertEqual(result['outcome'], 'PASS')
                self.assertEqual(len(result['sha256']), 64)
            campaign.fail = True
            with self.assertRaisesRegex(RuntimeError, 'job failed'):
                documents.capture(campaign, 'cma-migration', 'mongo', 'new-write', 'b' * 40, 'api')


if __name__ == '__main__':
    unittest.main()
