from contextlib import redirect_stdout
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import children
import gate
import selection
import version_label
from test_gates import needs
from test_policy import metadata
from policy import Workspace


class EntrypointTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.event = self.root / 'event.json'
        self.output = self.root / 'outputs'
        self.event.write_text(json.dumps({'pull_request': {'labels': []}}))
        self.environment = {'GITHUB_REPOSITORY': 'example/repository', 'GITHUB_EVENT_PATH': str(self.event), 'GITHUB_EVENT_NAME': 'pull_request', 'GITHUB_OUTPUT': str(self.output)}

    def test_selector_writes_one_complete_schema_only_after_success(self):
        result = {'repo': True, 'am': False, 'eg': False, 'web': True, 'am_packages': [], 'eg_packages': [], 'test_tracks': {'repo': True, 'am': False, 'eg': False, 'web': True}, 'deployables': ['cma-web'], 'base': 'a' * 40, 'head': 'b' * 40}
        with patch.dict(os.environ, self.environment), patch('sys.argv', ['selection']), redirect_stdout(io.StringIO()), patch.object(selection, 'determine', return_value=result):
            self.assertEqual(selection.main(), 0)
        outputs = dict(line.split('=', 1) for line in self.output.read_text().splitlines())
        self.assertEqual(json.loads(outputs['deployables']), ['cma-web'])
        self.assertEqual(outputs['head'], 'b' * 40)
        self.assertEqual(outputs['web'], 'true')
        self.output.unlink()
        with patch.dict(os.environ, self.environment), patch('sys.argv', ['selection']), redirect_stdout(io.StringIO()), patch.object(selection, 'determine', side_effect=ValueError('unsafe input')):
            self.assertEqual(selection.main(), 1)
        self.assertFalse(self.output.exists())

    def test_label_entrypoint_requires_selector_success(self):
        self.addCleanup(patch.stopall)
        patch.object(version_label, 'current_pull', side_effect=lambda event, repository: event['pull_request']).start()
        environment = dict(self.environment, SELECTOR_RESULT='success', DEPLOYABLES='[]')
        with patch.dict(os.environ, environment), redirect_stdout(io.StringIO()):
            self.assertEqual(version_label.main(), 0)
        for changes in [{'SELECTOR_RESULT': 'failure'}, {'DEPLOYABLES': '["cma-web"]'}, {'GITHUB_EVENT_NAME': 'push'}, {'DEPLOYABLES': 'not-json'}]:
            with patch.dict(os.environ, dict(environment, **changes)), redirect_stdout(io.StringIO()):
                self.assertEqual(version_label.main(), 1)
        self.event.write_text(json.dumps({'pull_request': {'labels': [{'name': 'version-upgrade:patch'}]}}))
        stream = io.StringIO()
        with patch.dict(os.environ, environment), redirect_stdout(stream):
            self.assertEqual(version_label.main(), 0)
        self.assertIn('ignored', stream.getvalue())

    def test_gate_entrypoints_propagate_failure(self):
        for status, expected in [('success', 0), ('cancelled', 1)]:
            value = needs('repo')
            value['version-label']['result'] = status
            with patch.dict(os.environ, NEEDS=json.dumps(value)), patch('sys.argv', ['gate', 'repo']), redirect_stdout(io.StringIO()):
                self.assertEqual(gate.main(), expected)
        value = {key: {'result': 'success'} for key in ['lint', 'tests', 'mongo', 'timing', 'isolation']}
        for status, expected in [('success', 0), ('skipped', 1)]:
            value['mongo']['result'] = status
            with patch.dict(os.environ, TRACK='am', NEEDS=json.dumps(value)), redirect_stdout(io.StringIO()):
                self.assertEqual(children.main(), expected)

    def test_analyze_combines_old_ownership_with_current_suite(self):
        current = Workspace(metadata(self.root), self.root)
        older = Workspace(metadata(self.root), self.root)
        # Simulate an old dependency edge selecting AM for a now-removed input.
        older.reverse['cma-image-manager'].add('cma')
        snapshots = [(self.root, older, {}), (self.root, current, {})]
        with patch.object(selection, 'changed_images', return_value=[]):
            result = selection.analyze(['api/crates/cma-image-manager/src/lib.rs'], snapshots)
        self.assertTrue(result['am'])
        self.assertEqual(result['test_tracks']['repo'], True)
        self.assertEqual(result['deployables'], [])


if __name__ == '__main__':
    unittest.main()
