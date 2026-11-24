"""Reconciliation accepts only a complete identity-checked selector batch."""

from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from staging_cd_common import Refusal
import staging_cd_comparisons as comparisons
import staging_cd_queue as queue


class BatchBoundaryTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(__file__).resolve().parents[2]
        self.before, self.first, self.second = (letter * 40 for letter in 'abc')
        self.revisions = [self.first, self.second]
        self.rows = [
            {'base': self.before, 'head': self.first, 'deployables': ['cma-web'], 'reasons': ['web-source']},
            {'base': self.first, 'head': self.second, 'deployables': [], 'reasons': []},
        ]

    def invoke(self, rows, code=0, stderr="", metrics=None, budget=None, stdout=None):
        stdout = json.dumps(rows) if stdout is None else stdout
        response = SimpleNamespace(returncode=code, stdout=stdout, stderr=stderr)
        with patch.object(comparisons.subprocess, 'run', return_value=response) as runner, \
                patch.object(comparisons, 'comparison_parents',
                             return_value={self.first: self.before, self.second: self.first}), \
                patch.dict(queue.os.environ, GH_TOKEN='synthetic-not-a-credential'):
            try:
                return queue.batch_selection(self.root, self.revisions, metrics, budget)
            finally:
                self.command = runner.call_args.args[0] if runner.called else None
                if runner.called:
                    self.assertNotIn('GH_TOKEN', runner.call_args.kwargs['env'])

    def test_complete_batch_preserves_noop_and_changed_merge(self):
        self.assertEqual(self.invoke(self.rows), {
            self.first: {'deployables': ['cma-web'], 'selection_reasons': []},
            self.second: {'deployables': [], 'selection_reasons': []}})

    def test_missing_reordered_or_wrong_identity_is_rejected(self):
        for rows in (self.rows[:1], list(reversed(self.rows)),
                     [dict(self.rows[0], base=self.first), self.rows[1]],
                     [dict(self.rows[0], head=self.second), self.rows[1]], {}):
            with self.subTest(rows=rows), self.assertRaises(Refusal):
                self.invoke(rows)

    def test_unknown_duplicate_unsorted_or_wrong_type_deployables_are_rejected(self):
        for names in (['unknown'], ['cma-web', 'cma-web'], ['cma-web', 'cma-agent-manager'], [1], None):
            with self.subTest(names=names), self.assertRaises(Refusal):
                self.invoke([dict(self.rows[0], deployables=names), self.rows[1]])

    def test_failed_selector_cannot_emit_successful_partial_results(self):
        with self.assertRaises(Refusal):
            self.invoke(self.rows, code=1)

    def test_an_uncached_batch_never_reaches_the_shared_cache(self):
        # A batch of one plan anchor must not prune the cached window's records.
        response = SimpleNamespace(returncode=0, stdout=json.dumps(self.rows), stderr='')
        for cached in (True, False):
            with self.subTest(cached=cached), patch.dict(queue.os.environ, CD_SELECTOR_CACHE='{}'), \
                    patch.object(comparisons.subprocess, 'run', return_value=response) as runner, \
                    patch.object(comparisons, 'comparison_parents',
                                 return_value={self.first: self.before, self.second: self.first}):
                comparisons.batch_selection(self.root, self.revisions, cached=cached)
            self.assertEqual('CD_SELECTOR_CACHE' in runner.call_args.kwargs['env'], cached)
            self.assertEqual(runner.call_args.kwargs['env'].keys() - {'CD_SELECTOR_CACHE'},
                             {key for key in ('PATH', 'HOME', 'LANG', 'LC_ALL', 'RUSTUP_HOME', 'CARGO_HOME')
                              if key in queue.os.environ})

    def test_reconcile_uses_batch_without_coalescing_selected_sources(self):
        def command(_root, *args):
            if args[0] == 'rev-list':
                return '\n'.join([self.second, self.first, self.before])
            return 'd' * 40
        with patch.object(queue, 'batch_selection', return_value={
                revision: {'deployables': ['cma-web']} for revision in self.revisions}) as batch:
            result = queue.reconcile(self.root, self.first, self.second, command=command)
        batch.assert_called_once_with(self.root, self.revisions, None, None)
        self.assertEqual([row['source'] for row in result], self.revisions)

    def test_conservative_reason_is_retained_in_each_plan_change(self):
        names = sorted(queue.DEPLOYABLES.values())
        rows = [dict(row, deployables=names, reasons=['input-analysis-uncertain']) for row in self.rows]
        decisions = self.invoke(rows)
        def command(_root, *args):
            return '\n'.join([self.second, self.first]) if args[0] == 'rev-list' else 'd' * 40
        with patch.object(queue, 'batch_selection', return_value=decisions):
            result = queue.reconcile(self.root, self.first, self.second, command=command)
        self.assertEqual([row['source'] for row in result], self.revisions)
        for row in result:
            self.assertEqual(row['deployables'], names)
            self.assertEqual(row['selection_reasons'], ['input-analysis-uncertain'])

    def test_fallback_cannot_omit_images_or_publish_raw_reasons(self):
        for reasons in (['shallow-history'], ['untrusted message\nvalue'], None):
            with self.assertRaises(Refusal):
                self.invoke([dict(self.rows[0], reasons=reasons), self.rows[1]])



    def test_metrics_project_only_bounded_numeric_fields_after_valid_selection(self):
        expected = {'hits': 1, 'misses': 1, 'input_hits': 1, 'input_misses': 1, 'elapsed_ms': 123}
        stats = {'event': 'selection-cache', **expected, 'untrusted': 'must not escape'}
        metrics = {}
        self.invoke(self.rows, stderr=json.dumps(stats), metrics=metrics)
        self.assertEqual(metrics, expected)
        for text in ('arbitrary subprocess output', '[]', json.dumps(dict(stats, hits=True)),
                     json.dumps(dict(stats, misses=-1)), json.dumps(dict(stats, elapsed_ms=100000001))):
            metrics = {}
            self.invoke(self.rows, stderr=text, metrics=metrics)
            self.assertEqual(metrics, {})
        metrics = {}
        with self.assertRaises(Refusal):
            self.invoke(self.rows, code=1, stderr=json.dumps(stats), metrics=metrics)
        self.assertEqual(metrics, {})

    def test_budget_reaches_the_selector_and_a_stopped_batch_reports_only_its_progress(self):
        stats = {'event': 'selection-cache', 'hits': 0, 'misses': 2, 'input_hits': 0, 'input_misses': 1,
                 'elapsed_ms': 2100000, 'decided': 1, 'total': 2}
        metrics = {}
        with self.assertRaises(comparisons.SelectionIncomplete) as stopped:
            self.invoke(None, code=comparisons.INCOMPLETE, stdout='', stderr=json.dumps(stats),
                        metrics=metrics, budget=2100)
        self.assertEqual(self.command[-1], '2100')
        self.assertEqual((stopped.exception.decided, stopped.exception.total), (1, 2))
        self.assertEqual(metrics, {key: stats[key] for key in comparisons.STATS})
        # Without the budget no flag is passed, so the same status is a plain failure.
        self.invoke(self.rows)
        self.assertNotIn('2100', self.command)
        with self.assertRaises(Refusal):
            self.invoke(None, code=comparisons.INCOMPLETE, stdout='', stderr=json.dumps(stats))

    def test_stopped_batch_cannot_claim_success_or_impossible_progress(self):
        stats = {'event': 'selection-cache', 'elapsed_ms': 1, 'decided': 1, 'total': 2}
        for stdout, values in (('[]', {}), ('', {'decided': 2}), ('', {'decided': 3}), ('', {'total': 3}),
                               ('', {'decided': True}), ('', {'decided': -1}), ('', {'total': None})):
            with self.subTest(stdout=stdout, values=values), self.assertRaises(Refusal):
                self.invoke(None, code=comparisons.INCOMPLETE, stdout=stdout,
                            stderr=json.dumps(dict(stats, **values)), budget=2100)
        for budget in (0, 2400, True, '60'):
            with self.subTest(budget=budget), self.assertRaises(Refusal):
                self.invoke(self.rows, budget=budget)

    def test_stopped_plan_saves_progress_then_the_gate_fails_with_the_catalogued_reason(self):
        with tempfile.TemporaryDirectory() as directory:
            outputs = Path(directory) / 'outputs'
            outputs.write_text('')
            env = {'GITHUB_OUTPUT': str(outputs), 'GITHUB_EVENT_NAME': 'push', 'GITHUB_SHA': self.second}
            stdout = io.StringIO()
            with patch.dict(queue.os.environ, env), patch.object(queue, 'event_identity'), \
                    patch.object(queue, 'git', return_value=self.second), \
                    patch.object(queue, 'release_queue', side_effect=comparisons.SelectionIncomplete(1, 2)) as plan, \
                    redirect_stdout(stdout):
                # The plan step passes, so the following cache save keeps its records.
                self.assertEqual(queue.main([]), 0)
            self.assertEqual(plan.call_args.kwargs['budget'], queue.BUDGET_SECONDS)
            # A budget stop is transient: catch-up may re-run it, from the first gate.
            self.assertEqual(outputs.read_text(), 'complete=false\ntransient=true\n')
            self.assertIn('INCOMPLETE release-queue decided=1 total=2 budget_seconds=2100\n', stdout.getvalue())
            stdout = io.StringIO()
            with patch.dict(queue.os.environ, {'GITHUB_STEP_SUMMARY': ''}), redirect_stdout(stdout):
                self.assertEqual(queue.main(['--require-complete']), 1)
            lines = stdout.getvalue().splitlines()
            self.assertEqual(lines[0], 'FAIL release-queue reason=incomplete cause=transient')
            self.assertIn('Release plan incomplete', lines[1])
            with redirect_stdout(io.StringIO()):
                self.assertEqual(queue.main(['--unexpected']), 1)

    def test_complete_plan_marks_itself_complete(self):
        with tempfile.TemporaryDirectory() as directory:
            outputs = Path(directory) / 'outputs'
            outputs.write_text('')
            env = {'GITHUB_OUTPUT': str(outputs), 'GITHUB_EVENT_NAME': 'push', 'GITHUB_SHA': self.second}
            result = {'head': self.second, 'event_source': self.second, 'changes': []}
            with patch.dict(queue.os.environ, env), patch.object(queue, 'event_identity'), \
                    patch.object(queue, 'git', return_value=self.second), \
                    patch.object(queue, 'release_queue', return_value=result), redirect_stdout(io.StringIO()):
                self.assertEqual(queue.main([]), 0)
            self.assertEqual(outputs.read_text().splitlines()[1:], ['changed=false', 'complete=true'])


if __name__ == '__main__':
    unittest.main()
