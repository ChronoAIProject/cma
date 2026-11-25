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

    def test_unsuccessful_selection_never_reaches_labels_or_passes(self):
        # The workflow suppresses superseded jobs; the validator remains strict
        # if an active job receives cancelled, missing or otherwise invalid input.
        for event_name in ('pull_request', 'merge_group', 'push'):
            for result in ('failure', 'cancelled', 'skipped', '', 'unknown', None):
                environment = dict(self.environment, GITHUB_EVENT_NAME=event_name)
                if result is None:
                    environment.pop('SELECTOR_RESULT')
                else:
                    environment['SELECTOR_RESULT'] = result
                output = io.StringIO()
                with self.subTest(event=event_name, result=result), \
                        patch.dict(os.environ, environment, clear=True), \
                        patch.object(version_label.subprocess, 'run') as request, \
                        redirect_stdout(output):
                    self.assertEqual(version_label.main(), 1)
                    request.assert_not_called()
                    self.assertNotIn('Version policy:', output.getvalue())

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

    def test_release_pr_bypasses_upgrade_only_at_current_develop_to_main_identity(self):
        self.pull['base']['ref'] = 'main'
        self.pull['head']['ref'] = 'develop'
        self.event['pull_request'] = copy.deepcopy(self.pull)
        code, output = self.check([], self.pull)
        self.assertEqual(code, 0)
        self.assertIn('not applicable', output)
        for side, field, value in [('base', 'ref', 'develop'), ('head', 'ref', 'feature'),
                                   ('head', 'sha', 'c' * 40), ('base', 'sha', 'd' * 40),
                                   ('head', 'repo', {'full_name': 'fork/repository'})]:
            current = copy.deepcopy(self.pull)
            current[side][field] = value
            with self.subTest(side=side, field=field):
                self.assertEqual(self.check([], current)[0], 1)
        self.assertEqual(self.check([], dict(self.pull, state='closed'))[0], 1)

    def queue(self, current, comparison=None, *, event=None, images=()):
        """Run the label check for develop merge-queue entry pr-7 with live PR responses."""
        group = {'base_sha': 'c' * 40, 'head_sha': 'd' * 40, 'base_ref': 'refs/heads/develop',
                 'head_ref': 'refs/heads/gh-readonly-queue/develop/pr-7-' + 'c' * 40}
        payload = event or {'action': 'checks_requested', 'repository': {'full_name': self.repository},
                            'merge_group': group}
        self.event_path.write_text(json.dumps(payload))
        contained = {'status': 'ahead', 'ahead_by': 1, 'behind_by': 0}
        responses = [current, contained if comparison is None else comparison]
        results = [subprocess.CompletedProcess([], 0, json.dumps(value), '') for value in responses]
        output = io.StringIO()
        environment = dict(self.environment, GITHUB_EVENT_NAME='merge_group', DEPLOYABLES=json.dumps(list(images)))
        with patch.dict(os.environ, environment), \
                patch.object(version_label.subprocess, 'run', side_effect=results) as request, \
                redirect_stdout(output):
            code = version_label.main()
        return code, output.getvalue(), [call.args[0][2] for call in request.call_args_list]

    def test_merge_group_requires_the_queued_prs_live_label(self):
        for images in [(), ('cma-web',)]:
            for level in ['major', 'minor', 'patch']:
                current = dict(self.pull, labels=[{'name': 'type:bug'}, {'name': f'version-upgrade:{level}'}])
                with self.subTest(images=images, level=level):
                    code, output, paths = self.queue(current, images=images)
                    self.assertEqual(code, 0)
                    self.assertIn('Queued PR: #7', output)
                    self.assertIn(f'Version policy: {level}', output)
                    self.assertEqual(paths, [f'repos/{self.repository}/pulls/7',
                                             f'repos/{self.repository}/compare/{"b" * 40}...{"d" * 40}'])
        for labels in [[], [{'name': 'version-upgrade:unsupported'}],
                       [{'name': 'version-upgrade:major'}, {'name': 'version-upgrade:minor'}]]:
            with self.subTest(labels=labels):
                code, output, _ = self.queue(dict(self.pull, labels=labels))
                self.assertEqual(code, 1)
                self.assertIn('::error::Version label check failed:', output)
                self.assertNotIn('Version policy:', output)

    def test_merge_group_label_must_belong_to_the_open_develop_pr_inside_the_group(self):
        current = dict(self.pull, labels=[{'name': 'version-upgrade:patch'}])
        for changes in [{'state': 'closed'}, {'number': 8}, {'labels': None},
                        {'base': dict(self.pull['base'], ref='main')},
                        {'base': dict(self.pull['base'], repo={'full_name': 'another/repository'})},
                        {'head': dict(self.pull['head'], sha='bad')}]:
            with self.subTest(changes=changes):
                self.assertEqual(self.queue(dict(current, **changes))[0], 1)
        # A head pushed after queueing, or a squash/rebase group, is not contained.
        for comparison in [{'status': 'diverged', 'ahead_by': 1, 'behind_by': 1},
                           {'status': 'behind', 'ahead_by': 0, 'behind_by': 2}, {}]:
            with self.subTest(comparison=comparison):
                code, output, paths = self.queue(current, comparison)
                self.assertEqual(code, 1)
                self.assertEqual(len(paths), 2)
                self.assertNotIn('Version policy:', output)

    def test_merge_group_identity_is_checked_before_any_live_read(self):
        group = {'base_sha': 'c' * 40, 'head_sha': 'd' * 40, 'base_ref': 'refs/heads/develop',
                 'head_ref': 'refs/heads/gh-readonly-queue/develop/pr-7-' + 'c' * 40}
        valid = {'action': 'checks_requested', 'repository': {'full_name': self.repository}, 'merge_group': group}
        current = dict(self.pull, labels=[{'name': 'version-upgrade:patch'}])
        cases = [dict(valid, action='destroyed'), dict(valid, repository={'full_name': 'other/repository'})]
        for field, value in [('base_ref', 'refs/heads/main'), ('head_sha', 'c' * 40), ('head_sha', '0' * 40),
                             ('base_sha', 'short'), ('head_ref', 'refs/heads/gh-readonly-queue/main/pr-7-' + 'c' * 40),
                             ('head_ref', 'gh-readonly-queue/develop/pr-7-' + 'c' * 40),
                             ('head_ref', 'refs/heads/gh-readonly-queue/develop/pr-0-' + 'c' * 40),
                             ('head_ref', 'refs/heads/gh-readonly-queue/develop/pr-7'), ('head_ref', None)]:
            cases.append(dict(valid, merge_group=dict(group, **{field: value})))
        for event in cases:
            with self.subTest(event=event):
                code, output, paths = self.queue(current, event=event)
                self.assertEqual((code, paths), (1, []))
                self.assertIn('::error::Version label check failed:', output)

    def test_push_policy_bypass_requires_valid_event_and_successful_selector(self):
        push = {'repository': {'full_name': self.repository}, 'ref': 'refs/heads/develop',
                'before': 'a' * 40, 'after': 'b' * 40, 'deleted': False}
        self.event_path.write_text(json.dumps(push))
        environment = dict(
            self.environment, GITHUB_EVENT_NAME='push', GITHUB_REF='refs/heads/develop',
            GITHUB_SHA='b' * 40, DEPLOYABLES='[]',
        )
        for changes, expected in [({}, 0), ({'SELECTOR_RESULT': 'failure'}, 1),
                                   ({'SELECTOR_RESULT': 'skipped'}, 1), ({'DEPLOYABLES': 'null'}, 1),
                                   ({'DEPLOYABLES': '["unknown"]'}, 1), ({'GITHUB_EVENT_NAME': 'workflow_run'}, 1),
                                   ({'GITHUB_EVENT_NAME': 'schedule', 'GITHUB_REF': 'refs/heads/main'}, 1),
                                   ({'GITHUB_REPOSITORY': 'other/repository'}, 1)]:
            with self.subTest(changes=changes), patch.dict(os.environ, dict(environment, **changes)), \
                    patch.object(version_label.subprocess, 'run') as request, redirect_stdout(io.StringIO()):
                self.assertEqual(version_label.main(), expected)
                request.assert_not_called()
        for field, value in [('ref', 'refs/heads/main'), ('deleted', True), ('after', '0' * 40),
                              ('before', 'bad'), ('repository', None)]:
            self.event_path.write_text(json.dumps(dict(push, **{field: value})))
            with patch.dict(os.environ, environment), redirect_stdout(io.StringIO()):
                self.assertEqual(version_label.main(), 1)


if __name__ == '__main__':
    unittest.main()
