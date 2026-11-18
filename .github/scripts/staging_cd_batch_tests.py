"""Reconciliation accepts only a complete identity-checked selector batch."""

import json
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from staging_cd_common import Refusal
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

    def invoke(self, rows, code=0):
        response = SimpleNamespace(returncode=code, stdout=json.dumps(rows))
        with patch.object(queue.subprocess, 'run', return_value=response) as runner, \
                patch.object(queue, 'comparison_parent', side_effect=[self.before, self.first]), \
                patch.dict(queue.os.environ, GH_TOKEN='synthetic-not-a-credential'):
            result = queue.batch_selection(self.root, self.revisions)
        self.assertNotIn('GH_TOKEN', runner.call_args.kwargs['env'])
        return result

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

    def test_reconcile_uses_batch_without_coalescing_selected_sources(self):
        def command(_root, *args):
            if args[0] == 'rev-list':
                return '\n'.join([self.second, self.first, self.before])
            return 'd' * 40
        with patch.object(queue, 'batch_selection', return_value={
                revision: {'deployables': ['cma-web']} for revision in self.revisions}) as batch:
            result = queue.reconcile(self.root, self.first, self.second, command=command)
        batch.assert_called_once_with(self.root, self.revisions)
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


if __name__ == '__main__':
    unittest.main()
