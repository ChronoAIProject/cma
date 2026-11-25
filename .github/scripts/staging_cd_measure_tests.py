"""Contract tests: privacy, failed samples, overlap and comparison equivalence."""
import argparse
import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

import staging_cd_measure as m


def args(**changes):
    values = dict(revision='a' * 40, workload='b' * 64, contract='c' * 64,
                  runner='local', cache='warm', component='shared', phase='plan', task_count=1,
                  repeats=3, timeout=1, cwd=str(Path(__file__).parent),
                  command=[sys.executable, '-B', '-c', 'pass'], timer_stage=None)
    return argparse.Namespace(**(values | changes))


class Contracts(unittest.TestCase):
    def test_projection_is_closed(self):
        row = {'event': 'publication-stage', 'stage': 'build', 'seconds': 12,
               'result': 'success', 'secret': 'PRIVATE_CANARY'}
        data = m.parse_events(['\x1b[32m2026-11-19T00:00:00Z ' + json.dumps(row)])
        self.assertNotIn('PRIVATE_CANARY', json.dumps(data))
        self.assertEqual(data['stage_seconds'], {'build': 12})
        self.assertNotIn('scan', data['stage_seconds'])

    def test_invalid_numbers_and_labels(self):
        rows = [{'event': 'publication-stage', 'stage': stage, 'seconds': seconds, 'result': 'success'}
                for stage, seconds in [('build', -1), ('build', True), ('build', float('nan')),
                                       ('build', float('inf')), ([], 1), ('untrusted', 1)]]
        data = m.parse_events(map(json.dumps, rows))
        self.assertFalse(data['events'])
        self.assertFalse(data['all_emitted_stages_successful'])

    def test_repeated_tasks_and_archive_not_added(self):
        rows = [{'event': 'publication-stage', 'stage': 'build', 'seconds': 10, 'result': 'success'},
                {'event': 'publication-stage', 'stage': 'build', 'seconds': 8, 'result': 'failure'},
                {'event': 'publication-build', 'complete': True, 'category_seconds':
                 {'archive-export': 4, 'rust-compile': 7, 'unknown': 99}}]
        data = m.parse_events(map(json.dumps, rows))
        self.assertEqual(data['stage_seconds']['build'], 18)
        self.assertFalse(data['all_emitted_stages_successful'])
        self.assertEqual(data['build_categories'][0]['category_seconds'], {'archive-export': 4, 'rust-compile': 7})

    def test_inverted_or_missing_timestamps(self):
        self.assertIsNone(m.seconds(None, None))
        self.assertIsNone(m.seconds('2026-11-19T01:00:01Z', '2026-11-19T01:00:00Z'))
        self.assertEqual(m.seconds('2026-11-19T01:00:00Z', '2026-11-19T01:00:09Z'), 9)

    def test_invalid_timer_magnitude_and_clock_shape(self):
        self.assertFalse(m.number(10**1000))
        self.assertIsNone(m.seconds('invalid', '2026-11-19T00:00:00Z'))
        self.assertIsNone(m.seconds('2026-11-19T00:00:00', '2026-11-19T00:00:00Z'))

    def test_local_success_does_not_retain_output_or_command(self):
        result = m.local(args(command=[sys.executable, '-B', '-c', 'print("PRIVATE_CANARY")']))
        self.assertTrue(result['successful'])
        self.assertEqual(len(result['samples']), 3)
        self.assertNotIn('PRIVATE_CANARY', json.dumps(result))

    def test_failure_stops_and_cannot_be_speedup(self):
        result = m.local(args(command=[sys.executable, '-B', '-c', 'raise SystemExit(7)']))
        self.assertFalse(result['successful'])
        self.assertIsNone(result['metric_seconds'])
        self.assertEqual(len(result['samples']), 1)

    def test_timeout_fails_closed(self):
        result = m.local(args(timeout=.03, command=[sys.executable, '-B', '-c', 'import time; time.sleep(3)']))
        self.assertTrue(result['samples'][0]['timed_out'])
        self.assertFalse(result['successful'])

    def compare_pair(self, mutation=None):
        before = {'mode': 'local', 'identity': m.identity(args()), 'successful': True,
                  'metric': 'median command wall seconds', 'metric_seconds': 10,
                  'samples': [{}, {}, {}], 'command_sha256': 'd' * 64}
        after = copy.deepcopy(before); after['metric_seconds'] = 8
        after['identity']['revision'] = 'e' * 40
        if mutation:
            mutation(after)
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as directory:
            p = Path(directory)
            (p / 'before.json').write_text(json.dumps(before)); (p / 'after.json').write_text(json.dumps(after))
            return m.compare(argparse.Namespace(before=str(p / 'before.json'), after=str(p / 'after.json')))

    def test_comparison_seconds(self):
        result = self.compare_pair()
        self.assertEqual(result['saving_seconds'], 2)
        self.assertEqual(result['saving_percent'], 20)

    def test_comparison_rejects_mismatched_workload_or_safety(self):
        for key in ['workload', 'contract', 'cache', 'runner', 'task_count']:
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.compare_pair(lambda x: x['identity'].update({key: 'different'}))

    def test_comparison_rejects_failed_or_small_sample(self):
        for mutation in [lambda x: x.update(successful=False), lambda x: x.update(samples=[{}])]:
            with self.assertRaises(ValueError):
                self.compare_pair(mutation)

    def test_ci_rejects_wrong_source_and_skipped_is_not_zero(self):
        job = {'head_sha': 'a' * 40, 'labels': ['ubuntu-24.04'], 'run_id': 1,
               'name': 'plan', 'status': 'completed', 'conclusion': 'skipped',
               'started_at': '2026-11-19T00:00:00Z', 'completed_at': '2026-11-19T00:00:00Z'}
        auth = argparse.Namespace(returncode=0, stdout=b'account chronoai-shining ', stderr=b'')
        a = args(runner='github-free', repo='ChronoAIProject/cma', job=1, step=None, logs=False)
        with patch.object(m.subprocess, 'run', return_value=auth), patch.object(m, 'gh_json', return_value=job):
            result = m.ci(a)
            self.assertIsNone(result['observed_seconds']); self.assertIsNone(result['metric_seconds'])
            a.revision = 'f' * 40
            with self.assertRaises(ValueError):
                m.ci(a)

    def test_ci_ignores_successful_conclusion_on_incomplete_record(self):
        job = {'head_sha': 'a' * 40, 'labels': ['ubuntu-24.04'], 'run_id': 1,
               'name': 'plan', 'status': 'in_progress', 'conclusion': 'success',
               'started_at': '2026-11-19T00:00:00Z', 'completed_at': '2026-11-19T00:00:01Z'}
        auth = argparse.Namespace(returncode=0, stdout=b'account chronoai-shining ', stderr=b'')
        a = args(runner='github-free', repo='ChronoAIProject/cma', job=1, step=None, logs=False)
        with patch.object(m.subprocess, 'run', return_value=auth), patch.object(m, 'gh_json', return_value=job):
            result = m.ci(a)
        self.assertFalse(result['successful'])
        self.assertIsNone(result['metric_seconds'])

    def test_ci_timer_requires_success_and_complete_task_count(self):
        job = {'head_sha': 'a' * 40, 'labels': ['ubuntu-24.04'], 'run_id': 1,
               'name': 'Publish', 'status': 'completed', 'conclusion': 'success',
               'started_at': '2026-11-19T00:00:00Z', 'completed_at': '2026-11-19T00:00:10Z'}
        auth = argparse.Namespace(returncode=0, stdout=b'account chronoai-shining ', stderr=b'')
        logs = argparse.Namespace(returncode=0, stderr=b'', stdout=json.dumps(
            {'event': 'publication-stage', 'stage': 'build', 'seconds': 6, 'result': 'success'}).encode())
        a = args(runner='github-free', repo='ChronoAIProject/cma', job=1, step=None, logs=True, timer_stage='build')
        with patch.object(m.subprocess, 'run', side_effect=[auth, logs]), patch.object(m, 'gh_json', return_value=job):
            self.assertEqual(m.ci(a)['metric_seconds'], 6)
        a.task_count = 2
        with patch.object(m.subprocess, 'run', side_effect=[auth, logs]), patch.object(m, 'gh_json', return_value=job):
            self.assertFalse(m.ci(a)['successful'])

    def test_ci_rejects_step_metric_with_sibling_publication_timer(self):
        selected = {'name': 'Plan', 'status': 'completed', 'conclusion': 'success',
                    'started_at': '2026-11-19T00:00:00Z', 'completed_at': '2026-11-19T00:00:03Z'}
        sibling = {'name': 'Publish', 'status': 'completed', 'conclusion': 'success',
                   'started_at': '2026-11-19T00:00:03Z', 'completed_at': '2026-11-19T00:00:10Z'}
        job = {'head_sha': 'a' * 40, 'labels': ['ubuntu-24.04'], 'run_id': 1,
               'name': 'Release', 'status': 'completed', 'conclusion': 'success',
               'started_at': selected['started_at'], 'completed_at': sibling['completed_at'],
               'steps': [selected, sibling]}
        auth = argparse.Namespace(returncode=0, stdout=b'account chronoai-shining ', stderr=b'')
        timer = json.dumps({'event': 'publication-stage', 'stage': 'build',
                            'seconds': 6, 'result': 'success'})
        logs = argparse.Namespace(returncode=0, stderr=b'', stdout=(
            '2026-11-19T00:00:01Z Planning complete\n2026-11-19T00:00:09Z ' + timer).encode())
        options = dict(runner='github-free', repo='ChronoAIProject/cma', job=1, logs=True)
        with patch.object(m.subprocess, 'run', side_effect=[auth, logs]) as run, \
                patch.object(m, 'gh_json', return_value=job) as get_job:
            with self.assertRaisesRegex(ValueError, '--step and --timer-stage cannot be combined'):
                m.ci(args(**options, step='Plan', timer_stage='build'))
            run.assert_not_called()
            get_job.assert_not_called()
            planned = m.ci(args(**options, step='Plan'))
        self.assertEqual(planned['metric'], 'step wall seconds')
        self.assertEqual(planned['metric_seconds'], 3)
        self.assertEqual(planned['phases']['stage_seconds'], {'build': 6})
        with patch.object(m.subprocess, 'run', side_effect=[auth, logs]), \
                patch.object(m, 'gh_json', return_value=job):
            publication = m.ci(args(**options, step=None, timer_stage='build'))
        self.assertEqual(publication['metric_seconds'], 6)

    def test_cli_does_not_overwrite_evidence(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as directory:
            out = Path(directory) / 'existing.json'; out.write_text('original')
            argv = ['local', '--out', str(out), '--revision', 'a'*40, '--workload', 'b'*64,
                    '--contract', 'c'*64, '--runner', 'local', '--cache', 'warm',
                    '--component', 'shared', '--phase', 'plan', '--cwd', str(Path(__file__).parent),
                    '--repeats', '1', '--', sys.executable, '-B', '-c', 'pass']
            self.assertEqual(m.main(argv), 2)
            self.assertEqual(out.read_text(), 'original')


if __name__ == '__main__':
    unittest.main()
