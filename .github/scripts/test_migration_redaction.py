import base64
import json
from pathlib import Path
import secrets
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import quote

import migration_catalog as catalog
import migration_kind_runner as runner
from migration_redaction import EvidenceRedactor


class MigrationRedactionTests(unittest.TestCase):
    def test_all_generated_manifest_credentials_are_removed_from_pod_diagnostics(self):
        passwords = [secrets.token_urlsafe(24) for _ in range(3)]
        key = base64.b64encode(secrets.token_bytes(32)).decode()
        credentials = {name: secrets.token_urlsafe(24) for name in (
            'client_id', 'client_secret', 'bearer', 'admin_bearer', 'outsider_bearer', 'refresh_token')}
        credentials['expires_at'] = 2000000000
        objects = runner.manifests('local-api', *passwords[:2], key, 'local-mongo', {},
                                   identity_credentials=credentials)
        objects = catalog.infrastructure(objects, 'cma-migration', passwords[2])
        redactor = EvidenceRedactor()
        redactor.collect(objects)
        retained = redactor.text(json.dumps(objects))
        for value in passwords + [key] + [credentials[name] for name in credentials
                                        if name not in ('client_id', 'expires_at')]:
            self.assertNotIn(value, retained)
        init = next(obj for obj in objects if obj.get('metadata', {}).get('name') == 'mongo-init'
                    and obj['kind'] == 'ConfigMap')
        for name in ('keyfile', 'control-key'):
            self.assertNotIn(init['data'][name], retained)
        self.assertIn('local-api', retained)
        self.assertIn(credentials['client_id'], retained)

    def test_redacts_json_and_uri_encodings_without_changing_other_evidence(self):
        value = secrets.token_urlsafe(24) + '"/\\+ '
        redactor = EvidenceRedactor()
        redactor.collect({'CHRONO_MONGODB_URI': 'mongodb://matrix:' + quote(value, safe='') + '@mongo/'})
        for encoded in (value, json.dumps(value)[1:-1], quote(value, safe='')):
            self.assertEqual(redactor.text('prefix ' + encoded + ' suffix'),
                             'prefix [REDACTED] suffix')
        self.assertEqual(redactor.text(b'ordinary output'), 'ordinary output')

    def test_invalid_identity_configuration_fails_before_execution(self):
        with self.assertRaises(json.JSONDecodeError):
            EvidenceRedactor().collect({'data': {'identity.json': '{'}})


class MigrationCommandEvidenceTests(unittest.TestCase):
    def campaign(self, directory):
        plan = Path(directory) / 'plan.json'
        plan.write_text('{}')
        campaign = runner.Campaign(directory, 'kind-not-invoked', 1, plan.read_bytes())
        campaign.images = ['local-api']
        credential = secrets.token_urlsafe(24)
        objects = [{'kind': 'Deployment', 'metadata': {'name': 'api'}, 'spec': {
            'env': [{'name': 'MATRIX_RUNTIME_PASSWORD', 'value': credential}]}}]
        with patch.object(campaign, 'command', return_value=''), \
                patch.object(campaign, 'reader_pods', return_value={
                    'metadata': {'resourceVersion': 'before-api'}, 'items': []}):
            campaign.create(objects)
        return campaign, credential

    def assert_retained_safe(self, directory, credential, campaign):
        for path in Path(directory).rglob('*'):
            if path.is_file():
                self.assertNotIn(credential, path.read_text())
        self.assertNotIn(credential, json.dumps(campaign.commands))
        self.assertIn('[REDACTED]', (Path(directory) / 'command-0000.private.log').read_text())

    def test_success_keeps_raw_output_in_memory_and_redacts_saved_output(self):
        with tempfile.TemporaryDirectory() as directory:
            campaign, credential = self.campaign(directory)
            program = 'import sys; print(sys.stdin.read())'
            raw = campaign.command([sys.executable, '-c', program],
                                   input_text=json.dumps({'password': credential, 'ready': True}))
            self.assertEqual(json.loads(raw), {'password': credential, 'ready': True})
            self.assertEqual(campaign.commands[0]['exit'], 0)
            self.assert_retained_safe(directory, credential, campaign)

    def test_nonzero_exit_preserves_failure_and_redacts_stderr(self):
        with tempfile.TemporaryDirectory() as directory:
            campaign, credential = self.campaign(directory)
            program = 'import sys; print(sys.stdin.read(), file=sys.stderr); sys.exit(7)'
            with self.assertRaises(runner.RunnerError):
                campaign.command([sys.executable, '-c', program], input_text=credential)
            self.assertEqual(campaign.commands[0]['exit'], 7)
            self.assert_retained_safe(directory, credential, campaign)

    def test_timeout_preserves_partial_diagnostics_and_joins_process(self):
        with tempfile.TemporaryDirectory() as directory:
            campaign, credential = self.campaign(directory)
            program = 'import sys, time; print(sys.stdin.read(), flush=True); time.sleep(30)'
            with self.assertRaisesRegex(runner.RunnerError, 'command timed out'):
                campaign.command([sys.executable, '-c', program], input_text=credential, timeout=1)
            self.assertTrue(campaign.commands[0]['timed_out'])
            self.assertIsNone(campaign.commands[0]['exit'])
            self.assert_retained_safe(directory, credential, campaign)

    def test_command_arguments_are_redacted_in_saved_metadata(self):
        with tempfile.TemporaryDirectory() as directory:
            campaign, credential = self.campaign(directory)
            process = subprocess.CompletedProcess(['fixture'], 0, credential)
            with patch.object(runner.subprocess, 'run', return_value=process):
                self.assertEqual(campaign.command(['fixture', credential]), credential)
            self.assertEqual(campaign.commands[0]['argv'], ['fixture', '[REDACTED]'])
            self.assert_retained_safe(directory, credential, campaign)


if __name__ == '__main__':
    unittest.main()
