"""Exercise the required check's exit status using the current PR response."""

from contextlib import redirect_stdout
import copy
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import version_label


class VersionLabelContractTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.event_path = Path(directory.name) / 'event.json'
        self.repository = 'example/repository'
        self.pull = {
            'number': 7, 'state': 'open', 'labels': [],
            'base': {'ref': 'develop', 'sha': 'a' * 40, 'repo': {'full_name': self.repository}},
            'head': {'ref': 'feature', 'sha': 'b' * 40, 'repo': {'full_name': self.repository}},
        }
        self.event = {'number': 7, 'repository': {'full_name': self.repository},
                      'pull_request': copy.deepcopy(self.pull)}
        self.environment = {
            'GITHUB_EVENT_NAME': 'pull_request', 'GITHUB_EVENT_PATH': str(self.event_path),
            'GITHUB_REPOSITORY': self.repository, 'SELECTOR_RESULT': 'success',
        }

    def check(self, images, current):
        self.event_path.write_text(json.dumps(self.event))
        output = io.StringIO()
        result = subprocess.CompletedProcess([], 0, json.dumps(current), '')
        with patch.dict(os.environ, dict(self.environment, DEPLOYABLES=json.dumps(images))), \
                patch.object(version_label.subprocess, 'run', return_value=result) as request, \
                redirect_stdout(output):
            code = version_label.main()
        request.assert_called_once_with(
            ['gh', 'api', f'repos/{self.repository}/pulls/7', '-H', 'Accept: application/vnd.github+json'],
            check=True, capture_output=True, text=True, timeout=30)
        return code, output.getvalue()

    def test_empty_selection_cannot_turn_missing_or_invalid_live_labels_green(self):
        # A previously labeled event must not hide removal or conflicting labels.
        self.event['pull_request']['labels'] = [{'name': 'version-upgrade:patch'}]
        for images in [[], ['cma-web']]:
            for labels in [[], [{'name': 'type:bug'}],
                           [{'name': 'version-upgrade:unsupported'}],
                           [{'name': 'version-upgrade:major'}, {'name': 'version-upgrade:minor'}],
                           [{'name': 'version-upgrade:patch'}, {'name': 'version-upgrade:unsupported'}]]:
                current = dict(self.pull, labels=labels)
                with self.subTest(images=images, labels=labels):
                    code, output = self.check(images, current)
                    self.assertEqual(code, 1)
                    self.assertIn('::error::Version label check failed:', output)
                    self.assertNotIn('Version policy:', output)

    def test_each_single_valid_live_label_passes_even_when_event_has_no_label(self):
        for images in [[], ['cma-web']]:
            for level in ['major', 'minor', 'patch']:
                current = dict(self.pull, labels=[{'name': 'type:bug'}, {'name': f'version-upgrade:{level}'}])
                with self.subTest(images=images, level=level):
                    code, output = self.check(images, current)
                    self.assertEqual(code, 0)
                    self.assertIn(f'Version policy: {level}', output)

    def test_empty_selection_still_fails_on_stale_or_malformed_live_pr(self):
        current = dict(self.pull, labels=[{'name': 'version-upgrade:patch'}])
        for changes in [{'state': 'closed'}, {'number': 8}, {'labels': None},
                        {'head': dict(self.pull['head'], sha='c' * 40)},
                        {'base': dict(self.pull['base'], repo={'full_name': 'another/repository'})}]:
            with self.subTest(changes=changes):
                code, output = self.check([], dict(current, **changes))
                self.assertEqual(code, 1)
                self.assertIn('::error::Version label check failed:', output)

    def test_empty_selection_still_fails_on_live_label_api_failure(self):
        self.event_path.write_text(json.dumps(self.event))
        for failure in [subprocess.CalledProcessError(1, ['gh']), subprocess.TimeoutExpired(['gh'], 30)]:
            with self.subTest(failure=type(failure).__name__), \
                    patch.dict(os.environ, dict(self.environment, DEPLOYABLES='[]')), \
                    patch.object(version_label.subprocess, 'run', side_effect=failure), \
                    redirect_stdout(io.StringIO()):
                self.assertEqual(version_label.main(), 1)


if __name__ == '__main__':
    unittest.main()
