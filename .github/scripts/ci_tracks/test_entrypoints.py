from contextlib import redirect_stdout
import copy
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
from test_gates import EXPECTED, needs
from test_policy import metadata
from policy import Workspace


class EntrypointTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.event = self.root / 'event.json'
        self.output = self.root / 'outputs'
        self.payload = {'number': 7, 'repository': {'full_name': 'example/repository'}, 'pull_request': {
            'labels': [],
            'base': {'ref': 'develop', 'sha': 'a' * 40, 'repo': {'full_name': 'example/repository'}},
            'head': {'ref': 'feature', 'sha': 'b' * 40, 'repo': {'full_name': 'example/repository'}}}}
        self.event.write_text(json.dumps(self.payload))
        self.environment = {'GITHUB_REPOSITORY': 'example/repository', 'GITHUB_EVENT_PATH': str(self.event), 'GITHUB_EVENT_NAME': 'pull_request', 'GITHUB_OUTPUT': str(self.output)}

    def test_selector_writes_one_complete_schema_only_after_success(self):
        result = {'repo': True, 'am': False, 'eg': False, 'web': True, 'am_packages': [], 'eg_packages': [], 'test_tracks': {'repo': True, 'am': False, 'eg': False, 'web': True}, 'deployables': ['cma-web'], 'base': 'a' * 40, 'head': 'b' * 40, 'reasons': ['web-source']}
        with patch.dict(os.environ, self.environment), patch('sys.argv', ['selection', '--ci']), redirect_stdout(io.StringIO()), patch.object(selection, 'determine', return_value=result), patch.object(selection, 'changed_paths', return_value=['web/src/agent/conversation.ts']):
            self.assertEqual(selection.main(), 0)
        outputs = dict(line.split('=', 1) for line in self.output.read_text().splitlines())
        self.assertEqual(json.loads(outputs['deployables']), ['cma-web'])
        self.assertEqual(outputs['head'], 'b' * 40)
        self.assertEqual(outputs['web'], 'true')
        self.assertEqual(outputs['approval_contract'], 'true')
        self.output.unlink()
        with patch.dict(os.environ, self.environment), patch('sys.argv', ['selection', '--ci']), redirect_stdout(io.StringIO()), patch.object(selection, 'determine', side_effect=ValueError('unsafe input')):
            self.assertEqual(selection.main(), 1)
        self.assertFalse(self.output.exists())

    def test_label_entrypoint_requires_selector_success(self):
        self.addCleanup(patch.stopall)
        patch.object(version_label, 'current_pull', side_effect=lambda event, repository: event['pull_request']).start()
        environment = dict(self.environment, SELECTOR_RESULT='success', DEPLOYABLES='[]')
        with patch.dict(os.environ, environment), redirect_stdout(io.StringIO()):
            self.assertEqual(version_label.main(), 1)
        for changes in [{'SELECTOR_RESULT': 'failure'}, {'DEPLOYABLES': '["cma-web"]'}, {'GITHUB_EVENT_NAME': 'push'}, {'DEPLOYABLES': 'not-json'}]:
            with patch.dict(os.environ, dict(environment, **changes)), redirect_stdout(io.StringIO()):
                self.assertEqual(version_label.main(), 1)
        self.payload['pull_request']['labels'] = [{'name': 'version-upgrade:patch'}]
        self.event.write_text(json.dumps(self.payload))
        stream = io.StringIO()
        with patch.dict(os.environ, environment), redirect_stdout(stream):
            self.assertEqual(version_label.main(), 0)
        self.assertIn('Version policy: patch', stream.getvalue())

    def test_gate_entrypoints_propagate_failure(self):
        for status, expected in [('success', 0), ('cancelled', 1)]:
            value = needs('repo')
            value['version-label']['result'] = status
            with patch.dict(os.environ, dict(self.environment, NEEDS=json.dumps(value))), patch('sys.argv', ['gate', 'repo']), redirect_stdout(io.StringIO()):
                self.assertEqual(gate.main(), expected)
        value = {key: {'result': 'success'} for key in ['lint', 'tests', 'mongo', 'timing', 'isolation']}
        for status, expected in [('success', 0), ('skipped', 1)]:
            value['mongo']['result'] = status
            with patch.dict(os.environ, TRACK='am', NEEDS=json.dumps(value)), redirect_stdout(io.StringIO()):
                self.assertEqual(children.main(), expected)

    def full_events(self):
        release = copy.deepcopy(self.payload)
        release['pull_request']['base']['ref'] = 'main'
        release['pull_request']['head']['ref'] = 'develop'
        push = {'repository': self.payload['repository'], 'ref': 'refs/heads/develop',
                'before': 'a' * 40, 'after': 'b' * 40, 'deleted': False}
        return [(release, 'pull_request'), (push, 'push')]

    def test_full_event_selector_cli_outputs_complete_suites(self):
        for payload, name in self.full_events():
            self.event.write_text(json.dumps(payload))
            base = dict(repo=True, am=False, eg=False, web=True, am_packages=[], eg_packages=[],
                        test_tracks=dict(repo=True, am=False, eg=False, web=True),
                        deployables=['cma-web'], head='b' * 40, base='a' * 40, reasons=['web-source'])
            with patch.dict(os.environ, dict(self.environment, GITHUB_EVENT_NAME=name)), \
                    patch('sys.argv', ['selection', '--ci']), redirect_stdout(io.StringIO()), \
                    patch.object(selection, 'determine', return_value=base), \
                    patch.object(selection, 'snapshot') as snapshot, \
                    patch.object(selection, 'workspace', return_value=Workspace(metadata(self.root), self.root)):
                snapshot.return_value.__enter__.return_value = self.root
                self.assertEqual(selection.main(), 0)
            outputs = dict(line.split('=', 1) for line in self.output.read_text().splitlines())
            for track in ('repo', 'am', 'eg', 'web'):
                self.assertEqual(outputs[track], 'true')
            self.assertEqual(json.loads(outputs['test_tracks']), dict(repo=True, am=True, eg=True, web=True))
            self.assertEqual(json.loads(outputs['deployables']), ['cma-web'])
            self.assertIn('layer4-runtime', json.loads(outputs['eg_packages']))
            self.output.unlink()

    def test_full_event_gates_reject_partial_selection_and_unsuccessful_children(self):
        for payload, name in self.full_events():
            self.event.write_text(json.dumps(payload))
            for track, jobs in EXPECTED.items():
                cases = [(needs(track), 0), (needs(track, False), 1)]
                for job in jobs | {'changes'}:
                    for status in ('skipped', 'cancelled', 'failure', None):
                        value = needs(track)
                        value[job]['result'] = status
                        cases.append((value, 1))
                    value = needs(track)
                    del value[job]
                    cases.append((value, 1))
                for value, expected in cases:
                    with self.subTest(event=name, track=track, needs=value), \
                            patch.dict(os.environ, dict(self.environment, GITHUB_EVENT_NAME=name, NEEDS=json.dumps(value))), \
                            patch('sys.argv', ['gate', track]), redirect_stdout(io.StringIO()):
                        self.assertEqual(gate.main(), expected)

    def test_ci_entrypoints_reject_unknown_or_malformed_events(self):
        cases = [(self.payload, 'workflow_run'), (self.payload, 'workflow_dispatch'),
                 (self.payload, 'schedule'), ({}, 'push')]
        for payload, name in self.full_events():
            wrong_repo = copy.deepcopy(payload)
            wrong_repo['repository']['full_name'] = 'other/repository'
            cases.append((wrong_repo, name))
        for field, value in [('ref', 'refs/heads/main'), ('deleted', True), ('deleted', 'false'), ('after', '0' * 40), ('after', 'bad')]:
            bad = dict(self.full_events()[1][0], **{field: value})
            cases.append((bad, 'push'))
        for side, field, value in [('base', 'ref', 'other'), ('head', 'ref', 'feature'),
                                   ('head', 'repo', {'full_name': 'fork/repository'}), ('head', 'sha', '')]:
            bad = copy.deepcopy(self.full_events()[0][0])
            bad['pull_request'][side][field] = value
            cases.append((bad, 'pull_request'))
        for payload, name in cases:
            self.event.write_text(json.dumps(payload))
            environment = dict(
                self.environment, GITHUB_EVENT_NAME=name, GITHUB_REF='refs/heads/main',
                GITHUB_SHA='b' * 40, NEEDS=json.dumps(needs('repo')),
                SELECTOR_RESULT='success', DEPLOYABLES='[]',
            )
            with self.subTest(payload=payload, event=name), \
                    patch.dict(os.environ, environment), \
                    redirect_stdout(io.StringIO()):
                with patch('sys.argv', ['selection', '--ci']), patch.object(selection, 'determine') as determine:
                    self.assertEqual(selection.main(), 1)
                    determine.assert_not_called()
                    self.assertFalse(self.output.exists())
                with patch('sys.argv', ['gate', 'repo']):
                    self.assertEqual(gate.main(), 1)
                with patch.object(version_label, 'current_pull') as current:
                    self.assertEqual(version_label.main(), 1)
                    current.assert_not_called()

    def queue_event(self, **group):
        return {'action': 'checks_requested', 'repository': {'full_name': 'example/repository'},
                'merge_group': dict({'base_sha': 'a' * 40, 'head_sha': 'b' * 40, 'base_ref': 'refs/heads/develop',
                                     'head_ref': 'refs/heads/gh-readonly-queue/develop/pr-7-' + 'a' * 40}, **group)}

    def test_merge_group_selection_and_gates_stay_path_selective(self):
        self.event.write_text(json.dumps(self.queue_event()))
        environment = dict(self.environment, GITHUB_EVENT_NAME='merge_group')
        result = {'repo': True, 'am': False, 'eg': False, 'web': True, 'am_packages': [], 'eg_packages': [],
                  'test_tracks': {'repo': True, 'am': False, 'eg': False, 'web': True}, 'deployables': ['cma-web'],
                  'base': 'a' * 40, 'head': 'b' * 40, 'reasons': ['web-source']}
        with patch.dict(os.environ, environment), patch('sys.argv', ['selection', '--ci']), \
                redirect_stdout(io.StringIO()), patch.object(selection, 'determine', return_value=result) as determine, \
                patch.object(selection, 'workspace') as workspace, \
                patch.object(selection, 'changed_paths', return_value=['web/src/main.tsx']):
            self.assertEqual(selection.main(), 0)
            workspace.assert_not_called()
        self.assertEqual(determine.call_args.args[1:], (self.queue_event(), 'merge_group'))
        outputs = dict(line.split('=', 1) for line in self.output.read_text().splitlines())
        self.assertEqual((outputs['am'], outputs['web'], outputs['head']), ('false', 'true', 'b' * 40))
        self.assertNotIn('full-ci-event', outputs['reasons'])
        self.assertEqual(outputs['approval_contract'], 'false')
        for track in ('am', 'eg', 'web'):
            skipped = needs(track, False)
            for job in {'rust-test-build', 'runtime-images', 'omarchy'} & set(skipped):
                skipped[job]['result'] = 'success'
            for value, expected in [(needs(track), 0), (skipped, 0), (dict(skipped, **{track: {'result': 'failure'}}), 1)]:
                with self.subTest(track=track, needs=value), \
                        patch.dict(os.environ, dict(environment, NEEDS=json.dumps(value))), \
                        patch('sys.argv', ['gate', track]), redirect_stdout(io.StringIO()):
                    self.assertEqual(gate.main(), expected)

    def test_ci_entrypoints_reject_malformed_or_non_develop_merge_groups(self):
        cases = [dict(self.queue_event(), action='destroyed'),
                 dict(self.queue_event(), repository={'full_name': 'other/repository'}),
                 self.queue_event(base_ref='refs/heads/main'), self.queue_event(head_sha='a' * 40),
                 self.queue_event(head_sha='0' * 40), self.queue_event(base_sha='bad'),
                 self.queue_event(head_ref='refs/heads/gh-readonly-queue/main/pr-7-' + 'a' * 40),
                 self.queue_event(head_ref='refs/heads/feature'), {'repository': {'full_name': 'example/repository'}}]
        for payload in cases:
            self.event.write_text(json.dumps(payload))
            with self.subTest(payload=payload), \
                    patch.dict(os.environ, dict(self.environment, GITHUB_EVENT_NAME='merge_group', NEEDS=json.dumps(needs('repo')),
                                                SELECTOR_RESULT='success', DEPLOYABLES='[]')), \
                    redirect_stdout(io.StringIO()):
                with patch('sys.argv', ['selection', '--ci']), patch.object(selection, 'determine') as determine:
                    self.assertEqual(selection.main(), 1)
                    determine.assert_not_called()
                    self.assertFalse(self.output.exists())
                with patch('sys.argv', ['gate', 'repo']):
                    self.assertEqual(gate.main(), 1)
                with patch.object(version_label, 'queued_pull') as queued:
                    self.assertEqual(version_label.main(), 1)
                    queued.assert_not_called()

    def test_analyze_combines_old_ownership_with_current_suite(self):
        current = Workspace(metadata(self.root), self.root)
        older = Workspace(metadata(self.root), self.root)
        # Simulate an old dependency edge selecting AM for a now-removed input.
        older.reverse['cma-image-manager'].add('cma')
        snapshots = [(self.root, older, {}), (self.root, current, {})]
        with patch.object(selection, 'image_inputs', return_value={}), patch.object(selection, 'recipes', return_value={}), \
                patch.object(selection, 'changed_input_maps', return_value=[]):
            result = selection.analyze(['api/crates/cma-image-manager/src/lib.rs'], snapshots)
        self.assertTrue(result['am'])
        self.assertEqual(result['test_tracks']['repo'], True)
        self.assertEqual(result['deployables'], [])


if __name__ == '__main__':
    unittest.main()
