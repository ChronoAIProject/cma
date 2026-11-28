import json
from pathlib import Path
import secrets
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from run_ci import run


class RunOutcomeTests(unittest.TestCase):
    def run_suite(self, code, coverage):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        directory = Path(temporary.name)
        output = directory / 'result'
        source = 'a' * 40
        image = 'localhost/fixture@sha256:' + 'b' * 64
        ready = {'expected_api_image': image, 'runtime_images': {'base': image, 'omarchy': image},
                 'runtime_sources': {'base': source, 'omarchy': source}, 'run_id': 'fixture-run'}
        infrastructure = Mock(created=False, stage='two_am_initialized', failed_command=None,
                              command_outcomes=[])
        infrastructure.setup.return_value = ready
        infrastructure.cleanup.return_value = {'cluster_absent': True, 'children_joined': True}
        fixture = Mock(thread=None, server=None)
        fixture.origins = {'a': 'http://127.0.0.1:1234', 'b': 'http://127.0.0.1:5678'}
        fixture.secret = {name: secrets.token_urlsafe() for name in
                          ('bearer', 'admin_bearer', 'outsider_bearer')}
        fixture.control_token = secrets.token_urlsafe()
        fixture.browser_cookie = secrets.token_urlsafe()
        fixture.serve.return_value = 'http://127.0.0.1:9012'
        fixture.client.deadline = 0
        fixture.client.close.return_value = True

        def execute(command, **options):
            self.assertEqual(Path(command[1]).name, 'prepared_suite.mjs')
            self.assertFalse(options['check'])
            config = json.loads(options['data'])
            self.assertEqual(config['expected_api_source'], source)
            if coverage is not None:
                (Path(config['summary_dir']) / 'coverage.json').write_text(json.dumps(coverage))
            return code, ''

        fixture.client.command.side_effect = execute
        args = SimpleNamespace(revision=source, release_packet=None, packet_sha256=None,
                               artifacts=directory, run_id='1', attempt='1', fixture='prepared', output=output)
        with patch('run_ci.Infrastructure', return_value=infrastructure), \
                patch('run_ci.LocalFixture', return_value=fixture), \
                patch('run_ci.prepare_tls', return_value=directory / 'tls'), \
                patch('run_ci.signal.signal'), patch('run_ci.os.umask'), \
                patch('run_ci.summarize_fixture', return_value={}), \
                patch('run_ci.summarize_runtime_failure', return_value=None), \
                patch('run_ci.summarize_stages', return_value={}), patch('builtins.print'):
            status = run(args)

        result = json.loads((output / 'summary' / 'result.json').read_text())
        self.assertEqual(result['source'], source)
        self.assertEqual(result['stage'], 'feature_journey')
        self.assertEqual(result['suite_exit_code'], code)
        self.assertEqual(result['cleanup'], {'cluster_absent': True, 'children_joined': True})
        self.assertTrue(result['fixture_children_joined'])
        fixture.client.command.assert_called_once()
        fixture.client.close.assert_called_once_with()
        infrastructure.cleanup.assert_called_once_with()
        if coverage is not None:
            self.assertEqual(json.loads((output / 'summary' / 'coverage.json').read_text()), coverage)
        return status, result

    def test_successful_cluster_cleanup_does_not_erase_public_retirement_failure(self):
        coverage = {'suite_verdict': 'FAIL', 'journey_verdict': 'FAIL_CLEANUP',
                    'journey_cleanup_failures': {'public_retirement': 1, 'provider_release': 0,
                                                 'manager_restore': 0}}
        status, result = self.run_suite(1, coverage)
        self.assertEqual(status, 1)
        self.assertEqual(result['verdict'], 'FAIL')
        self.assertIsNone(result['infrastructure_failure'])

    def test_suite_process_and_coverage_must_both_pass(self):
        for code, verdict in [(0, 'FAIL'), (1, 'PASS'), (0, 'PASS')]:
            with self.subTest(code=code, verdict=verdict):
                status, result = self.run_suite(code, {'suite_verdict': verdict})
                passed = code == 0 and verdict == 'PASS'
                self.assertEqual(status, 0 if passed else 1)
                self.assertEqual(result['verdict'], 'PASS' if passed else 'FAIL')

    def test_zero_exit_without_coverage_fails_and_still_cleans_up(self):
        status, result = self.run_suite(0, None)
        self.assertEqual(status, 1)
        self.assertEqual(result['verdict'], 'FAIL')
        self.assertEqual(result['failure_code'], 'suite_coverage_artifact_missing')


if __name__ == '__main__':
    unittest.main()
