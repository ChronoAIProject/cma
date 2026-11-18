"""Batch selection equals independent comparisons and cannot omit history."""

from contextlib import redirect_stderr, redirect_stdout
import io
import json
import subprocess
import unittest
from unittest.mock import patch

import batch_selection
from policy import Workspace
import selection
from selection_cache import SelectionCache
import test_images
from test_policy import metadata


class BatchTests(unittest.TestCase):
    write = test_images.ImageTests.write
    pin_recipes = test_images.ImageTests.pin_recipes

    def setUp(self):
        test_images.ImageTests.setUp(self)
        self.git('init', '-q', '-b', 'develop')
        self.git('config', 'user.name', 'Synthetic Test')
        self.git('config', 'user.email', 'test@example.invalid')
        self.base = self.commit()
        self.write('web/src/main.tsx', 'export const app = 2;')
        self.first = self.commit()
        self.write('api/crates/cmaeg/src/lib.rs', '#[cfg(test)] mod tests;')
        self.write('api/crates/cmaeg/src/tests.rs', 'fn check() {}')
        self.second = self.commit()
        (self.root / 'web/src/main.tsx').unlink()
        self.third = self.commit()
        self.revisions = [self.first, self.second, self.third]
        self.workspace = patch.object(selection, 'workspace', side_effect=lambda tree: Workspace(metadata(tree), tree))
        self.workspace.start()
        self.addCleanup(self.workspace.stop)

    def git(self, *args):
        return subprocess.check_output(['git', *args], cwd=self.root, stderr=subprocess.PIPE, text=True).strip()

    def commit(self):
        self.git('add', '.')
        self.git('commit', '-qm', 'Synthetic selector change')
        return self.git('rev-parse', 'HEAD')

    def test_batch_preserves_each_change_and_analyses_only_n_plus_one_snapshots(self):
        expected = [selection.determine(self.root, {'after': revision}, 'push') for revision in self.revisions]
        with patch.object(batch_selection, 'summarize', wraps=selection.summarize) as summarize:
            actual = batch_selection.determine_many(self.root, self.revisions)
        self.assertEqual(actual, expected)
        self.assertEqual(summarize.call_count, len(self.revisions) + 1)
        self.assertIn('cma-web', actual[0]['deployables'])
        self.assertIn('cma-event-gateway', actual[1]['deployables'])
        self.assertIn('cma-web', actual[2]['deployables'])

    def test_batch_rejects_missing_reordered_duplicate_or_invalid_commits(self):
        for revisions in ([], [self.first, self.third], list(reversed(self.revisions)),
                          [self.first, self.first], ['--help'], [self.first] * 2001):
            with self.subTest(revisions=len(revisions)), self.assertRaises(ValueError):
                batch_selection.determine_many(self.root, revisions)

    def test_failed_mid_batch_analysis_emits_no_partial_success(self):
        batch = self.root / 'batch.json'
        batch.write_text(json.dumps(self.revisions))
        stdout, stderr = io.StringIO(), io.StringIO()
        with patch('sys.argv', ['batch', str(self.root), str(batch)]), \
                patch.object(batch_selection, 'summarize', side_effect=ValueError('private input')), \
                redirect_stdout(stdout), redirect_stderr(stderr):
            self.assertEqual(batch_selection.main(), 1)
        self.assertEqual(stdout.getvalue(), '')
        self.assertNotIn('private input', stderr.getvalue())

    def test_new_invocation_revalidates_changed_current_recipe(self):
        batch_selection.determine_many(self.root, [self.first])
        self.write('.github/scripts/deployables.json', '{}')
        with self.assertRaises(ValueError):
            batch_selection.determine_many(self.root, [self.first])

    def test_warm_cache_reuses_only_complete_comparisons_then_analyzes_new_source(self):
        cache = SelectionCache(self.root / 'cache', 'e' * 64)
        old = batch_selection.determine_many(self.root, self.revisions[:2], cache)
        with patch.object(batch_selection, 'summarize', wraps=selection.summarize) as summarize:
            actual = batch_selection.determine_many(self.root, self.revisions, cache)
        self.assertEqual(actual[:2], old)
        self.assertEqual(summarize.call_count, 2)
        self.assertIn('cma-web', actual[2]['deployables'])
        with patch.object(batch_selection, 'summarize', side_effect=AssertionError('must reuse')):
            self.assertEqual(batch_selection.determine_many(self.root, self.revisions, cache), actual)

    def test_corrupt_record_is_recomputed_without_losing_selection(self):
        cache = SelectionCache(self.root / 'cache', 'e' * 64)
        expected = batch_selection.determine_many(self.root, self.revisions, cache)
        cache.path(self.second).write_text('invalid')
        with patch.object(batch_selection, 'summarize', wraps=selection.summarize) as summarize:
            actual = batch_selection.determine_many(self.root, self.revisions, cache)
        self.assertEqual(actual, expected)
        self.assertEqual(summarize.call_count, 2)


if __name__ == '__main__':
    unittest.main()
