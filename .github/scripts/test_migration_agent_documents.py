from copy import deepcopy
import hashlib
import json
from pathlib import Path
import secrets
import tempfile
import unittest
from unittest.mock import patch

import migration_agent_documents as documents
from migration_native_campaign import NativeCampaignError, digest
from test_migration_native_campaign import artifacts


def snapshot():
    return {'schema': 'migration-agent-documents-v1', 'database': 'matrix',
            'collections': {name: [] for name in documents.COLLECTIONS},
            'document_hashes': {name: [] for name in documents.COLLECTIONS}, 'redactions': [],
            'consistency': documents.CONSISTENCY}


class DocumentsTests(unittest.TestCase):
    def infrastructure(self):
        return [
            {'kind': 'Job', 'metadata': {'name': 'mongo-init', 'namespace': 'cma-migration'},
             'spec': {'template': {'spec': {'containers': [{'env': []}]}}}},
            {'kind': 'ConfigMap', 'metadata': {'name': 'mongo-init', 'namespace': 'cma-migration'},
             'data': {'init.sh': ''}},
        ]

    def test_role_is_find_only_scoped_no_application_writer_or_secret(self):
        password = secrets.token_urlsafe(24)
        values = documents.infrastructure(self.infrastructure(), password)
        script = values[1]['data']['init.sh']
        privileges = json.loads(script.split('privileges:', 1)[1].split(',roles:[]', 1)[0])
        self.assertEqual([r['resource']['collection'] for r in privileges], list(documents.COLLECTIONS))
        for row in privileges:
            self.assertEqual(row['actions'], ['find', 'killCursors'])
            self.assertEqual(row['resource']['db'], 'matrix')
        self.assertNotIn(password, script)
        self.assertFalse(any(v['kind'] == 'Secret' for v in values))
        with self.assertRaises(NativeCampaignError): documents.infrastructure(values, password)
        for bad in (None, 'short', 'a'*80, "'"*32):
            with self.assertRaises(NativeCampaignError): documents.infrastructure(self.infrastructure(), bad)
        foreign = self.infrastructure();foreign[0]['metadata']['namespace'] = 'foreign'
        with self.assertRaises(NativeCampaignError): documents.infrastructure(foreign, password)

    def test_job_has_no_credential_argv_or_token_and_never_restarts(self):
        job = documents.job(artifacts()['old']['image'], 'old-write')
        self.assertEqual(job['spec']['backoffLimit'], 0)
        pod = job['spec']['template']['spec']
        self.assertFalse(pod['automountServiceAccountToken'])
        self.assertEqual(pod['restartPolicy'], 'Never')
        self.assertEqual(pod['containers'][0]['command'], ['mongosh', '--nodb', '--quiet', '/catalog/read.cjs'])
        self.assertEqual(pod['volumes'][0]['configMap']['name'], 'mongo-agent-documents-code')

    def test_validate_preserves_nested_shapes_and_rejects_partial_or_duplicate(self):
        value = snapshot();value['collections']['agents'] = [{'_id': 'a', 'record': {'nested': [None]}}]
        value['document_hashes']['agents'] = [{'id_sha256': 'a'*64, 'document_sha256': 'b'*64}]
        self.assertEqual(documents.validate(json.dumps(value)), value)
        for mutate in (
                lambda v: v.update(database='other'), lambda v: v.update(consistency='atomic'),
                lambda v: v['collections'].pop('agents'),
                lambda v: v['collections'].update(agents=[{}]),
                lambda v: v['collections'].update(agents=[{'_id': 'same'}, {'_id': 'same'}]),
                lambda v: v['collections'].update(agents=[{'_id': n} for n in range(2049)]),
                lambda v: v.update(collections={k: [{'_id': i} for i in range(500)] for k in documents.COLLECTIONS}),
        ):
            changed = deepcopy(value);mutate(changed)
            with self.assertRaises(NativeCampaignError): documents.validate(json.dumps(changed))
        with self.assertRaises(NativeCampaignError): documents.validate(None)
        with patch.object(documents, 'MAX_BYTES', 2):
            with self.assertRaises(NativeCampaignError): documents.validate(json.dumps(value))

    def test_capture_bound_to_reader_and_journal_private_exclusive_file(self):
        with tempfile.TemporaryDirectory() as directory:
            class Campaign:
                output = Path(directory).resolve()
                calls = []
                def kubectl(self, args, **kwargs):
                    self.calls.append((args, kwargs))
                    return json.dumps(snapshot()) if 'logs' in args else ''
            campaign = Campaign()
            selected = artifacts()['old'];reader = {'uid': 'fixture-api'}
            session = {'stage': 'old-write', 'result': {'artifact': selected},
                       'journal': {'tail_sha256': 'a'*64}}
            result = documents.capture(campaign, selected['image'], 'old-write', selected, reader, session)
            path = Path(directory) / result['raw_path']
            self.assertEqual(result['reader_sha256'], digest(reader))
            self.assertEqual(result['raw_sha256'], hashlib.sha256(path.read_bytes()).hexdigest())
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertEqual(result['compatibility'], 'NOT_PROVEN')
            self.assertEqual(campaign.calls[0][0], ['create', '-f', '-'])
            with self.assertRaises(FileExistsError):
                documents.capture(campaign, selected['image'], 'old-write', selected, reader, session)
            # Reject scope before any collector Job starts.
            count = len(campaign.calls)
            for mutation in ({**session, 'stage': 'new-write'}, {**session, 'journal': {}}):
                with self.assertRaises(NativeCampaignError):
                    documents.capture(campaign, selected['image'], 'old-write', selected, reader, mutation)
            self.assertEqual(len(campaign.calls), count)

    def test_python_and_javascript_collection_allowlists_equal(self):
        import subprocess
        code = "console.log(JSON.stringify(require('./migration_agent_documents.cjs').COLLECTIONS))"
        result = subprocess.check_output(['node', '-e', code], cwd=Path(__file__).parent, text=True)
        self.assertEqual(json.loads(result), list(documents.COLLECTIONS))


if __name__ == '__main__':
    unittest.main()
