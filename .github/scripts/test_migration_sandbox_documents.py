"""Fail closed when command snapshots omit rows, rewrite history or widen authority."""
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import secrets
import tempfile
import unittest

import migration_sandbox_documents as docs
from migration_browser_fixture import OWNER
from migration_kind_manifests import manifests


def bson(value):
    if type(value) is int:
        return {'$numberLong': str(value)}
    if isinstance(value, dict):
        return {k: bson(v) for k, v in value.items()}
    if isinstance(value, list):
        return [bson(v) for v in value]
    return value


def digest(value):
    return hashlib.sha256(json.dumps(value, separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()


def snapshot(writers=1):
    records, controls = [], []
    for writer in range(writers):
        commands = []
        for i in range(2 * writer, 2 * writer + 2):
            identifier, sandbox = 'scmd_' + format(i, '032x'), 'sbx_' + format(i, '032x')
            key = 'matrix-key-' + str(i)
            admitted = '2026-01-01T00:00:00Z'
            deadline = '2026-01-01T00:05:00Z'
            state = {'status': 'rejected', 'at': '2026-01-01T00:00:01Z', 'reason': 'not_eligible'}
            receipt = {'command_id': identifier, 'sandbox_id': sandbox, 'command': 'pause',
                       'expected_revision': 3, 'admitted_at': admitted, 'queue_deadline': deadline,
                       'revision': 2, 'state': state}
            commands.append({'key': key, 'receipt': receipt})
            owner = {'kind': 'person', 'subject': OWNER}
            actor = {'kind': 'api', 'subject': OWNER, 'delegation_id': None}
            envelope = {k: receipt[k] for k in ('command_id', 'sandbox_id', 'command', 'expected_revision')}
            envelope.update(owner=owner, actor=actor, sandbox_created_at=admitted,
                            request_fingerprint=digest(['sandbox-control-v1', 'pause', 3]),
                            authorization={'authority_id': 'fixture-authority', 'policy_revision': 1,
                                           'grant_revision': None})
            audit = {k: envelope[k] for k in ('owner', 'actor', 'sandbox_id', 'sandbox_created_at')}
            audit.update(target={'kind': 'sandbox', 'sandbox_id': sandbox}, capability='manage_sandbox',
                         policy_revision=0, captured_at=admitted,
                         observations=[{'basis': {'kind': 'personal_owner'}, 'capabilities': ['manage_sandbox']}])
            sha = hashlib.sha256(key.encode()).hexdigest()
            record = {'schema_version': 1, 'envelope': envelope, 'key_sha256': sha,
                      'admitted_at': admitted, 'queue_deadline': deadline, 'status': state,
                      'revision': 2, 'admission_audit': audit, 'execution_audit': None}
            controls.append(bson({'_id': identifier, 'document_schema_version': 1, 'sandbox_id': sandbox,
                                 'scope': digest(['sandbox-control-scope-v1', 'fixture-authority',
                                                  actor, sandbox, 'pause']),
                                 'key_sha256': sha, 'queued': False, 'revision': 2,
                                 'queue_deadline': deadline, 'record': record}))
        records.append({'writer': 'old-write' if not writer else 'new-write', 'commands': commands})
    return {'schema': docs.SCHEMA, 'database': 'matrix', 'collections': {'sandbox_controls': controls}}, records


class SandboxDocumentsTests(unittest.TestCase):
    def test_browser_control_retains_api_actor_with_explicit_null_delegation(self):
        # resource_request_context records cookie-authenticated controls
        # as ResourceActor::Api. The visible user remains a personal owner.
        value, records = snapshot()
        docs.validate(value, records)
        actors = [
            {'kind': 'human', 'subject': OWNER},
            {'kind': 'api', 'subject': OWNER},
            {'kind': 'api', 'subject': OWNER, 'delegation_id': 'foreign'},
            {'kind': 'api', 'subject': 'foreign', 'delegation_id': None},
            {'kind': 'api', 'subject': OWNER, 'delegation_id': None, 'extra': True},
        ]
        for actor in actors:
            value, records = snapshot()
            row = value['collections']['sandbox_controls'][0]
            row['record']['envelope']['actor'] = actor
            row['record']['admission_audit']['actor'] = actor
            # A self-consistent scope cannot authorize another actor shape.
            row['scope'] = digest(['sandbox-control-scope-v1', 'fixture-authority',
                                   actor, row['sandbox_id'], 'pause'])
            with self.subTest(actor=actor), self.assertRaises(RuntimeError):
                docs.validate(value, records)

    def test_both_writers_join_actual_controls_and_embedded_audits(self):
        old, before = snapshot(); new, after = snapshot(2)
        docs.validate(old, before); docs.validate(new, after)
        docs.compare(old['collections'], deepcopy(old['collections']), False)
        docs.compare(old['collections'], new['collections'], True)
        docs.compare(new['collections'], deepcopy(new['collections']), False)

    def test_empty_duplicate_foreign_and_additional_collections_fail(self):
        for mutation in ('empty', 'duplicate', 'foreign', 'extra'):
            value, records = snapshot()
            rows = value['collections']['sandbox_controls']
            if mutation == 'empty': rows.clear()
            if mutation == 'duplicate': rows[1] = deepcopy(rows[0])
            if mutation == 'foreign': rows[0]['_id'] = 'foreign'
            if mutation == 'extra': value['collections']['auth_subjects'] = []
            with self.subTest(mutation=mutation), self.assertRaises(RuntimeError):
                docs.validate(value, records)

    def test_wrong_state_receipt_binding_actor_and_embedded_audit_fail(self):
        mutations = [lambda r: r.update(queued=True),
                     lambda r: r.update(revision=bson(1)),
                     lambda r: r.update(document_schema_version=bson(2)),
                     lambda r: r.update(key_sha256='b' * 64),
                     lambda r: r['record'].update(status={'status': 'queued'}),
                     lambda r: r['record']['envelope']['actor'].update(subject='foreign'),
                     lambda r: r['record']['admission_audit'].update(observations=[]),
                     lambda r: r['record'].update(execution_audit={})]
        for mutation in mutations:
            value, records = snapshot(); mutation(value['collections']['sandbox_controls'][0])
            with self.subTest(mutation=mutation), self.assertRaises(RuntimeError):
                docs.validate(value, records)

    def test_reader_rejects_unbound_idempotency_scope_and_fingerprint(self):
        mutations = [lambda r: r.update(scope='b' * 64),
                     lambda r: r['record']['envelope'].update(request_fingerprint='b' * 64),
                     lambda r: r['record']['envelope'].pop('request_fingerprint'),
                     lambda r: r['record']['envelope'].update(unreviewed_field=True)]
        for index, mutation in enumerate(mutations):
            value, records = snapshot(); mutation(value['collections']['sandbox_controls'][0])
            with self.subTest(index=index), self.assertRaises(RuntimeError):
                docs.validate(value, records)

    def test_reader_rejects_invalid_retained_authority_and_audit_shape(self):
        mutations = [lambda r: r['record']['envelope']['authorization'].update(policy_revision=True),
                     lambda r: r['record']['envelope']['authorization'].update(grant_revision=bson(0)),
                     lambda r: r['record']['envelope']['authorization'].update(authority_id=' '),
                     lambda r: r['record']['envelope']['authorization'].update(unreviewed_field=True),
                     lambda r: r['record']['admission_audit'].pop('captured_at'),
                     lambda r: r['record']['admission_audit'].update(captured_at='not-a-time'),
                     lambda r: r['record']['admission_audit'].update(policy_revision=True),
                     lambda r: r['record']['admission_audit'].update(unreviewed_field=True),
                     lambda r: r['record']['admission_audit']['observations'][0].update(
                         capabilities=['manage_sandbox', 'unknown_capability']),
                     lambda r: r['record']['admission_audit']['observations'].append(
                         {'basis': {'kind': 'global_operator'}, 'capabilities': ['manage_sandbox']})]
        for index, mutation in enumerate(mutations):
            value, records = snapshot(); mutation(value['collections']['sandbox_controls'][0])
            with self.subTest(index=index), self.assertRaises(RuntimeError):
                docs.validate(value, records)

    def test_reader_rejects_wrong_writer_provenance_and_unsigned_bson_overflow(self):
        value, records = snapshot(); records[0]['writer'] = 'new-write'
        with self.assertRaises(RuntimeError): docs.validate(value, records)
        for kind, maximum in (('$numberInt', 2**31 - 1), ('$numberLong', 2**63 - 1)):
            self.assertEqual(docs._integer({kind: str(maximum)}), maximum)
            with self.assertRaises(RuntimeError): docs._integer({kind: str(maximum + 1)})

    def test_reader_requires_canonical_numbers_and_valid_source_receipts(self):
        for field, bad in (('schema_version', True), ('schema_version', 1), ('revision', 2.0)):
            value, records = snapshot(); value['collections']['sandbox_controls'][0]['record'][field] = bad
            with self.subTest(field=field, bad=bad), self.assertRaises(RuntimeError):
                docs.validate(value, records)
        for bad in (None, {}, {'key': 'present', 'receipt': {}},
                    {'key': 'present', 'receipt': {'sandbox_id': 'foreign', 'expected_revision': 3}}):
            value, records = snapshot(); records[0]['commands'][0] = bad
            with self.subTest(bad=bad), self.assertRaises(RuntimeError): docs.validate(value, records)

    def test_timestamp_calendar_and_authority_grant_boundaries(self):
        value, records = snapshot()
        value['collections']['sandbox_controls'][0]['record']['envelope']['authorization']['grant_revision'] = bson(1)
        docs.validate(value, records)
        for captured in ('2026-02-30T00:00:00Z', '2026-01-01T00:00:00', '2026-01-01T00:00:00+24:00'):
            value, records = snapshot()
            value['collections']['sandbox_controls'][0]['record']['admission_audit']['captured_at'] = captured
            with self.subTest(captured=captured), self.assertRaises(RuntimeError): docs.validate(value, records)

    def test_no_rewrite_or_deletion_at_reader_or_writer_switch(self):
        old, _ = snapshot(); new, _ = snapshot(2)
        for writer, after in ((False, deepcopy(old['collections'])), (True, new['collections'])):
            after['sandbox_controls'][0]['scope'] = 'b' * 64
            with self.assertRaises(RuntimeError): docs.compare(old['collections'], after, writer)
        with self.assertRaises(RuntimeError): docs.compare(old['collections'], {'sandbox_controls': []}, True)

    def test_bson_identity_and_duplicate_api_receipts_fail(self):
        for value in (True, 1, {}, {'$numberDouble': '1'}, {'$numberLong': '-1'}, {'$numberLong': '01'}):
            with self.subTest(value=value), self.assertRaises(RuntimeError): docs._integer(value)
        for field, bad in (('schema', 'wrong'), ('database', 'admin')):
            value, records = snapshot(); value[field] = bad
            with self.assertRaises(RuntimeError): docs.validate(value, records)
        value, records = snapshot(); records[0]['commands'][1] = records[0]['commands'][0]
        with self.assertRaises(RuntimeError): docs.validate(value, records)

    def test_authority_is_exact_find_only_and_jobs_have_no_serviceaccount(self):
        objects = manifests('api', 'runtime', 'admin', 'key', 'mongo', {})
        password = secrets.token_urlsafe(24)
        docs.infrastructure(objects, 'cma-migration', password)
        code = next(o['data']['init.sh'] for o in objects if o['kind'] == 'ConfigMap'
                    and o['metadata']['name'] == 'mongo-init')
        privileges = json.loads(code.split('role:"matrix_sandbox_command_reader",privileges:')[1].split(',roles:[]')[0])
        self.assertEqual(privileges, [{'resource': {'db': 'matrix', 'collection': 'sandbox_controls'}, 'actions': ['find']}])
        job = docs.job('cma-migration', 'mongo', 'old-write')
        self.assertFalse(job['spec']['template']['spec']['automountServiceAccountToken'])
        self.assertEqual(job['spec']['backoffLimit'], 0)
        self.assertNotIn(password, json.dumps(job))
        self.assertFalse(any(o['kind'] == 'Secret' for o in objects))
        with self.assertRaises(ValueError): docs.infrastructure([], 'matrix', "bad'password")

    def test_capture_preserves_first_job_failure_and_binds_readers(self):
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
            (campaign.output / 'old-write-sandbox-controls.private.json').write_text(json.dumps(records[0]))
            for stage in ('old-write', 'new-read-old-write'):
                result = docs.capture(campaign, 'matrix', 'mongo', stage, 'a' * 40, 'api')
                self.assertEqual(result['commands'], 2)
                self.assertEqual(result['outcome'], 'PASS')
            campaign.fail = True
            with self.assertRaisesRegex(RuntimeError, 'job failed'):
                docs.capture(campaign, 'matrix', 'mongo', 'new-write', 'b' * 40, 'api')


if __name__ == '__main__':
    unittest.main()
