"""Only the trusted develop push workflow can prepare release selection caches."""

import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from staging_cd_common import Refusal
from staging_cd_selection_cache import prepare


class CacheAuthorityTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.event = self.root / 'event'
        self.event.write_text(json.dumps({'ref': 'refs/heads/develop', 'after': 'a' * 40,
                                         'deleted': False, 'forced': False,
                                         'repository': {'full_name': 'ChronoAIProject/cma'}}))
        self.env = {'GITHUB_EVENT_NAME': 'push', 'GITHUB_REF': 'refs/heads/develop',
                    'GITHUB_REPOSITORY': 'ChronoAIProject/cma', 'GITHUB_SHA': 'a' * 40,
                    'GITHUB_WORKFLOW_REF': 'ChronoAIProject/cma/.github/workflows/staging-cd.yml@refs/heads/develop',
                    'GITHUB_RUN_ID': '10', 'GITHUB_RUN_ATTEMPT': '1',
                    'GITHUB_EVENT_PATH': str(self.event), 'RUNNER_TEMP': str(self.root),
                    'GITHUB_OUTPUT': str(self.root / 'output'), 'GITHUB_ENV': str(self.root / 'environment')}

    def test_trusted_develop_context_binds_immutable_policy_and_attempt(self):
        with patch('staging_cd_selection_cache.fingerprint', return_value='b' * 64):
            self.assertTrue(prepare(self.root, self.env))
        outputs = dict(line.split('=', 1) for line in Path(self.env['GITHUB_OUTPUT']).read_text().splitlines())
        self.assertEqual(outputs['key'], outputs['prefix'] + 'a' * 40 + '-10-1')
        context = json.loads(Path(self.env['GITHUB_ENV']).read_text().split('=', 1)[1])
        self.assertEqual(context['policy'], 'b' * 64)
        self.assertEqual(context['directory'], str(self.root.resolve() / 'cma-selection-cache'))

    def test_pr_fork_other_ref_or_other_workflow_is_rejected_before_any_output(self):
        for changes in ({'GITHUB_EVENT_NAME': 'pull_request'}, {'GITHUB_EVENT_NAME': 'pull_request_target'},
                        {'GITHUB_REF': 'refs/pull/10/merge'}, {'GITHUB_REF': 'refs/heads/main'},
                        {'GITHUB_REPOSITORY': 'example/fork'},
                        {'GITHUB_WORKFLOW_REF': 'ChronoAIProject/cma/.github/workflows/ci.yml@refs/heads/develop'}):
            with self.subTest(changes=changes), self.assertRaises(Refusal):
                prepare(self.root, dict(self.env, **changes))
            self.assertFalse(Path(self.env['GITHUB_OUTPUT']).exists())
            self.assertFalse(Path(self.env['GITHUB_ENV']).exists())

    def test_event_payload_cannot_disagree_with_environment(self):
        value = json.loads(self.event.read_text())
        value['after'] = 'b' * 40
        self.event.write_text(json.dumps(value))
        with self.assertRaises(Refusal):
            prepare(self.root, self.env)


if __name__ == '__main__':
    unittest.main()
