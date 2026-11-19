"""Safe failure categories retain refusal gates and suppress private output."""
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import urllib.error
from unittest.mock import Mock, patch

from image_secret_gate import ScanRefusal
from staging_cd_build import command
from staging_cd_common import Refusal
from staging_cd_publication_diagnostics import ImageCommandRefusal, run_publication
import staging_cd_publish as publication
import staging_cd_publish_tests as fixtures
from staging_cd_receipts_tests import fixture_identity, fixture_receipt


class PublicationDiagnosticTests(unittest.TestCase):
    def observe(self, operation):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            status = run_publication(operation)
        return status, output.getvalue()

    def test_command_denials_classify_without_releasing_bytes(self):
        cases = [(b'DENIED: permission_denied: write_package private', 'registry_denied'),
                 (b'UNAUTHORIZED private', 'registry_denied'),
                 (b'unexpected status code 403 private', 'registry_denied'),
                 (b'private\xff\n::warning::injected', 'command_failed'),
                 (b'unexpected status code 503 private', 'command_failed')]
        for stderr, reason in cases:
            with self.subTest(reason=reason), patch('staging_cd_build.subprocess.run',
                    return_value=Mock(returncode=1, stdout=b'private stdout', stderr=stderr)) as runner:
                def operation(_masker, diagnostics):
                    diagnostics.mark('registry_copy')
                    command(['skopeo', 'copy', 'private-source', 'private-target'])
                self.assertEqual(self.observe(operation),
                                 (1, f'FAIL\nFAIL publication stage=registry_copy reason={reason}\n'))
                runner.assert_called_once()

    def test_build_timeout_and_failure_are_distinct_from_registry_denial(self):
        cases = [(subprocess.TimeoutExpired(['private-command'], 1, output=b'private'), 'build_timeout'),
                 (ImageCommandRefusal(['docker', 'buildx'], b'DENIED private'), 'build_failed')]
        for error, reason in cases:
            with self.subTest(reason=reason):
                def operation(_masker, diagnostics):
                    diagnostics.mark('build')
                    raise error
                self.assertEqual(self.observe(operation),
                                 (1, f'FAIL\nFAIL publication stage=build reason={reason}\n'))

    def test_http_reads_distinguish_denial_from_other_failures(self):
        for status, reason in [(401, 'registry_denied'), (403, 'registry_denied'), (503, 'registry_read_failed')]:
            with self.subTest(status=status):
                def operation(_masker, diagnostics):
                    diagnostics.mark('registry_preflight')
                    raise Refusal('private') from urllib.error.HTTPError('https://example.invalid', status,
                                                                         'private', {}, None)
                self.assertEqual(self.observe(operation),
                                 (1, f'FAIL\nFAIL publication stage=registry_preflight reason={reason}\n'))

    def test_actual_publish_scan_refusal_never_logs_in_or_copies(self):
        with tempfile.TemporaryDirectory() as directory:
            archive, digest, registry, _scanner, runner = fixtures.PublicationTests().fixture(directory)
            def operation(_masker, diagnostics):
                return publication.publish(archive, fixture_identity(), Path(directory), registry, 'fixture', 'actor',
                                           reservation_check=lambda: True, scanner=Mock(side_effect=ScanRefusal('private finding')),
                                           runner=runner, diagnostics=diagnostics)
            with patch.object(publication, 'inspect', return_value=digest):
                self.assertEqual(self.observe(operation), (1, 'FAIL\nFAIL publication stage=scan reason=scan_refused\n'))
            runner.assert_not_called()

    def test_actual_publish_copy_denial_is_one_attempt_and_cleans_auth(self):
        with tempfile.TemporaryDirectory() as directory:
            archive, digest, registry, scanner, runner = fixtures.PublicationTests().fixture(directory)
            login = runner.side_effect
            def denied(args, **kwargs):
                if args[:2] == ['skopeo', 'copy']:
                    raise ImageCommandRefusal(args, b'DENIED private registry body')
                return login(args, **kwargs)
            runner.side_effect = denied
            def operation(_masker, diagnostics):
                return publication.publish(archive, fixture_identity(), Path(directory), registry, 'fixture', 'actor',
                                           reservation_check=lambda: True, scanner=scanner, runner=runner, diagnostics=diagnostics)
            with patch.object(publication, 'inspect', return_value=digest):
                self.assertEqual(self.observe(operation),
                                 (1, 'FAIL\nFAIL publication stage=registry_copy reason=registry_denied\n'))
            self.assertEqual(runner.call_count, 2)
            registry.exists.assert_not_called()
            self.assertFalse((Path(directory) / 'registry-auth.json').exists())

    def test_new_attempt_refuses_old_reservation_before_build(self):
        identity = fixture_identity()
        reservation = {k: v for k, v in identity.items() if k != 'build'}
        reservation['schema'] = 'cma-version-reservation-v1'
        with tempfile.TemporaryDirectory() as directory:
            env = {'GITHUB_EVENT_NAME': 'push', 'GITHUB_RUN_ID': '100', 'GITHUB_RUN_ATTEMPT': '2',
                   'CD_COMPONENT': 'web', 'CD_RECEIPTS_PATH': str(Path(directory) / 'receipts.json'),
                   'CD_TASKS': json.dumps([{'component': 'web', 'mode': 'publish', 'reservation': reservation}])}
            output = io.StringIO()
            with patch.dict(os.environ, env, clear=True), patch.object(publication, 'event_identity'), \
                    patch.object(publication, 'Registry'), patch.object(publication, 'GitHub'), \
                    patch.object(publication, 'snapshot') as snapshot, contextlib.redirect_stdout(output):
                self.assertEqual(publication.main(), 1)
            snapshot.assert_not_called()
            self.assertFalse(Path(env['CD_RECEIPTS_PATH']).exists())
            self.assertEqual(output.getvalue(),
                             'FAIL\nFAIL publication stage=validate reason=reservation_owner_mismatch\n')

    def test_entrypoint_reports_build_timeout_and_persists_only_success(self):
        identity = fixture_identity()
        reservation = {k: v for k, v in identity.items() if k != 'build'}
        reservation['schema'] = 'cma-version-reservation-v1'
        for mode in ('timeout', 'receipt_failure', 'success'):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as directory:
                receipt_path = Path(directory) / 'receipts.json'
                env = {'GITHUB_EVENT_NAME': 'push', 'GITHUB_RUN_ID': '100', 'GITHUB_RUN_ATTEMPT': '1',
                       'RUNNER_TEMP': directory, 'CD_COMPONENT': 'web', 'CD_RECEIPTS_PATH': str(receipt_path),
                       'CD_TASKS': json.dumps([{'component': 'web', 'mode': 'publish', 'reservation': reservation}])}
                output = io.StringIO()
                with contextlib.ExitStack() as stack:
                    stack.enter_context(patch.dict(os.environ, env, clear=True))
                    for name in ('event_identity', 'Registry', 'GitHub', 'Reservations', 'snapshot'):
                        stack.enter_context(patch.object(publication, name))
                    stack.enter_context(patch.object(publication, 'identity', return_value=(identity, {}, {})))
                    stack.enter_context(patch.object(publication, 'try_reuse', return_value=None))
                    build = stack.enter_context(patch.object(publication, 'build',
                                                            return_value=(Path(directory) / 'image.tar', identity)))
                    publish = stack.enter_context(patch.object(publication, 'publish', return_value=fixture_receipt()))
                    if mode == 'timeout':
                        build.side_effect = subprocess.TimeoutExpired('private command', 1, output=b'private')
                    if mode == 'receipt_failure':
                        stack.enter_context(patch.object(Path, 'write_text', side_effect=OSError('private path')))
                    stack.enter_context(contextlib.redirect_stdout(output))
                    status = publication.main()
                if mode == 'success':
                    self.assertEqual((status, output.getvalue()), (0, 'PASS\n'))
                    self.assertEqual(json.loads(receipt_path.read_text()), [fixture_receipt()])
                else:
                    stage, reason = ('build', 'build_timeout') if mode == 'timeout' else ('receipt', 'io_failed')
                    self.assertEqual((status, output.getvalue()),
                                     (1, f'FAIL\nFAIL publication stage={stage} reason={reason}\n'))
                    self.assertFalse(receipt_path.exists())
                if mode == 'timeout':
                    publish.assert_not_called()

    def test_closed_fallbacks_and_success(self):
        cases = [(Refusal('private\n::warning::injected'), 'validation_refused'),
                 (OSError('private'), 'io_failed'), (ValueError('private'), 'unexpected_error'),
                 (subprocess.TimeoutExpired('private', 1), 'command_timeout')]
        for error, reason in cases:
            with self.subTest(reason=reason):
                def operation(_masker, diagnostics):
                    diagnostics.mark('receipt')
                    raise error
                self.assertEqual(self.observe(operation),
                                 (1, f'FAIL\nFAIL publication stage=receipt reason={reason}\n'))
        self.assertEqual(self.observe(lambda _masker, _diagnostics: True), (0, 'PASS\n'))
        def invalid(_masker, diagnostics):
            diagnostics.mark('private\n::warning::injected')
        self.assertEqual(self.observe(invalid), (1, 'FAIL\nFAIL publication stage=validate reason=validation_refused\n'))

    def test_native_and_child_outputs_remain_private(self):
        program = """
import os, subprocess, sys
from staging_cd_publication_diagnostics import ImageCommandRefusal, run_publication
def operation(_masker, diagnostics):
    diagnostics.mark('registry_copy')
    print('private python output')
    os.write(1, b'private native output')
    os.write(2, b'private native error')
    subprocess.run([sys.executable, '-c', "print('private child output')"], check=True)
    raise ImageCommandRefusal(['skopeo', 'copy'], b'DENIED private')
raise SystemExit(run_publication(operation))
"""
        result = subprocess.run([sys.executable, '-B', '-c', program], cwd=Path(__file__).parent,
                                capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, 'FAIL\nFAIL publication stage=registry_copy reason=registry_denied\n')
        self.assertEqual(result.stderr, '')


if __name__ == '__main__':
    unittest.main()
