"""Batch selection equals independent comparisons and cannot omit history."""

from contextlib import redirect_stderr, redirect_stdout
from hashlib import sha256
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import batch_selection
import image_inputs
from image_inputs import MANIFEST
from uncertainty import SelectionIntegrityError
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
        # Call counts are observed in this process; forked workers keep their own.
        self.sequential = patch.object(batch_selection, 'workers', return_value=1)
        self.sequential.start()
        self.addCleanup(self.sequential.stop)

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

    def test_parallel_summaries_match_sequential_results_and_cache(self):
        sequential = batch_selection.determine_many(self.root, self.revisions, parallel=1)
        for parallel in (2, 4):
            with self.subTest(parallel=parallel), tempfile.TemporaryDirectory() as directory:
                cache = SelectionCache(Path(directory), 'a' * 64)
                self.assertEqual(batch_selection.determine_many(self.root, self.revisions, cache, parallel), sequential)
                saved = sorted(path.name for path in Path(directory).iterdir())
                self.assertEqual([name for name in saved if not name.startswith('inputs-')],
                                 sorted(revision + '.json' for revision in self.revisions))
                warm = SelectionCache(Path(directory), 'a' * 64)
                with patch.object(batch_selection, 'summarize', side_effect=AssertionError('must reuse')):
                    self.assertEqual(batch_selection.determine_many(self.root, self.revisions, warm, parallel),
                                     sequential)
                self.assertEqual(warm.hits, len(self.revisions))

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

    def test_reviewed_historical_runtime_recipe_replays_after_restoring_the_pin(self):
        path = image_inputs.RECIPE
        original = (self.root / path).read_text()
        previous = sha256(original.encode()).hexdigest()
        edited = original + 'ENV SYNTHETIC_FEATURE=1\n'
        current = sha256(edited.encode()).hexdigest()
        self.write(path, edited)
        stale = self.commit()
        with patch.object(image_inputs, 'PRE_SEAL_RECIPE', previous), \
                patch.object(image_inputs, 'SEAL_RECIPE', current):
            with self.assertRaises(SelectionIntegrityError):
                batch_selection.determine_many(self.root, [stale])
            self.write(path, original)
            repair = self.commit()
            plans = batch_selection.determine_many(self.root, [stale, repair])
            self.assertEqual(plans[1], selection.determine(self.root, {'after': repair}, 'push'))
            self.assertEqual(plans[1]['deployables'], [
                'cma-agent-manager', 'cma-runtime', 'cma-runtime-omarchy',
            ])
            self.assertIn('cma-runtime', plans[0]['deployables'])
            with patch.object(image_inputs, 'SEAL_RECIPE', '0' * 64), \
                    self.assertRaises(SelectionIntegrityError):
                batch_selection.determine_many(self.root, [stale, repair])

    def test_web_recipe_manifest_repair_replays_the_stale_historical_revision(self):
        path = 'web/Dockerfile'
        previous = sha256((self.root / path).read_bytes()).hexdigest()
        self.write(path, (self.root / path).read_text() + 'ENV WEB_DRAIN_FIXTURE=1\n')
        current = sha256((self.root / path).read_bytes()).hexdigest()
        stale = self.commit()
        with self.assertRaises(SelectionIntegrityError):
            batch_selection.determine_many(self.root, [stale])
        manifest = self.root / '.github/scripts/deployables.json'
        self.write('.github/scripts/deployables.json', manifest.read_text().replace(previous, current))
        repaired = self.commit()
        with patch.object(image_inputs, 'PRE_WEB_DRAIN_RECIPE', previous), \
                patch.object(image_inputs, 'WEB_DRAIN_RECIPE', current):
            plans = batch_selection.determine_many(self.root, [stale, repaired])
            self.assertEqual([row['deployables'] for row in plans], [['cma-web'], ['cma-web']])
        with self.assertRaises(SelectionIntegrityError):
            batch_selection.determine_many(self.root, [stale, repaired])

    def test_warm_cache_reuses_only_complete_comparisons_then_analyzes_new_source(self):
        cache = SelectionCache(self.root / 'cache', 'e' * 64)
        old = batch_selection.determine_many(self.root, self.revisions[:2], cache)
        with patch.object(batch_selection, 'summarize', wraps=selection.summarize) as summarize:
            actual = batch_selection.determine_many(self.root, self.revisions, cache)
        self.assertEqual(actual[:2], old)
        self.assertEqual(summarize.call_count, 1)
        self.assertEqual(cache.input_hits, 1)
        self.assertIn('cma-web', actual[2]['deployables'])
        with patch.object(batch_selection, 'summarize', side_effect=AssertionError('must reuse')):
            self.assertEqual(batch_selection.determine_many(self.root, self.revisions, cache), actual)

    def test_budget_stops_between_snapshots_and_the_next_invocation_continues(self):
        expected = batch_selection.determine_many(self.root, self.revisions)
        cache = SelectionCache(self.root / 'cache', 'e' * 64)
        clock = iter([0.0, 0.0, 5.0])  # Two snapshots start before the deadline at 1.0.
        with patch.object(batch_selection.time, 'monotonic', side_effect=lambda: next(clock, 9.0)), \
                self.assertRaises(batch_selection.BudgetExhausted) as stopped:
            batch_selection.determine_many(self.root, self.revisions, cache, deadline=1.0)
        # The base and first head were analysed, so only the first comparison is decided.
        self.assertEqual((stopped.exception.decided, stopped.exception.total), (1, 3))
        self.assertEqual(sorted(path.name for path in cache.directory.iterdir()),
                         sorted([self.first + '.json', 'inputs-' + self.base + '.json',
                                 'inputs-' + self.first + '.json']))
        resumed = SelectionCache(self.root / 'cache', 'e' * 64)
        with patch.object(batch_selection, 'summarize', wraps=selection.summarize) as summarize:
            self.assertEqual(batch_selection.determine_many(self.root, self.revisions, resumed), expected)
        self.assertEqual((resumed.hits, resumed.input_hits, summarize.call_count), (1, 1, 2))

    def test_stopped_main_prints_only_progress_and_a_distinct_status(self):
        batch = self.root / 'batch.json'
        batch.write_text(json.dumps(self.revisions))
        context = json.dumps({'directory': str(self.root / 'cache'), 'policy': 'e' * 64})
        for argv, status in ((['batch', str(self.root), str(batch), '0'], 1),
                             (['batch', str(self.root), str(batch), 'x'], 1),
                             (['batch', str(self.root), str(batch), '60', 'extra'], 1)):
            with self.subTest(argv=argv), patch('sys.argv', argv), \
                    redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                self.assertEqual(batch_selection.main(), status)
        stdout, stderr = io.StringIO(), io.StringIO()
        with patch('sys.argv', ['batch', str(self.root), str(batch), '60']), \
                patch.dict('os.environ', CD_SELECTOR_CACHE=context), \
                patch.object(batch_selection, 'fingerprint', return_value='e' * 64), \
                patch.object(batch_selection, 'expired', side_effect=[False, True]), \
                redirect_stdout(stdout), redirect_stderr(stderr):
            self.assertEqual(batch_selection.main(), batch_selection.INCOMPLETE)
        self.assertEqual(stdout.getvalue(), '')
        stats = json.loads(stderr.getvalue())
        self.assertEqual((stats['event'], stats['decided'], stats['total']), ('selection-cache', 0, 3))
        self.assertEqual(set(stats), {'event', 'hits', 'misses', 'input_hits', 'input_misses',
                                      'elapsed_ms', 'decided', 'total'})

    def test_corrupt_record_is_recomputed_without_losing_selection(self):
        cache = SelectionCache(self.root / 'cache', 'e' * 64)
        expected = batch_selection.determine_many(self.root, self.revisions, cache)
        cache.path(self.second).write_text('invalid')
        with patch.object(batch_selection, 'summarize', wraps=selection.summarize) as summarize:
            actual = batch_selection.determine_many(self.root, self.revisions, cache)
        self.assertEqual(actual, expected)
        self.assertEqual(summarize.call_count, 2)



    def test_recipe_refresh_recomputes_historical_comparisons_and_selects_new_image(self):
        cache = SelectionCache(self.root / 'cache', 'e' * 64)
        expected = batch_selection.determine_many(self.root, self.revisions, cache)
        self.write('web/Dockerfile', (self.root / 'web/Dockerfile').read_text() + 'RUN true\n')
        self.pin_recipes()
        head = self.commit()
        with patch.object(batch_selection, 'summarize', wraps=selection.summarize) as summarize:
            actual = batch_selection.determine_many(self.root, self.revisions + [head], cache)
        self.assertEqual(actual[:-1], expected)
        self.assertEqual(cache.hits, 0)
        self.assertEqual(cache.input_hits, 0)
        self.assertEqual(summarize.call_count, len(self.revisions) + 2)
        self.assertIn('cma-web', actual[-1]['deployables'])
        self.assertEqual(actual[-1], selection.determine(self.root, {'after': head}, 'push'))

    def test_archived_manifest_exclusion_refresh_recomputes_comparisons_and_input_maps(self):
        self.write('.gitattributes', MANIFEST + ' export-ignore\n')
        parent = self.commit()
        (self.root / 'web/src/main.test.tsx').unlink()
        head = self.commit()

        # The first cache contains the decision; the second can supply only
        # its parent's input map. Both were computed with the old fallback.
        decisions = SelectionCache(self.root / 'decision-cache', 'e' * 64)
        maps = SelectionCache(self.root / 'map-cache', 'e' * 64)
        self.assertEqual(batch_selection.determine_many(self.root, [head], decisions)[0]['deployables'], [])
        batch_selection.determine_many(self.root, [parent], maps)
        self.manifest['cma-web']['broad_copy_exclusions'] = []
        self.pin_recipes()
        expected = batch_selection.determine_many(self.root, [head])
        self.assertEqual(expected[0]['deployables'], ['cma-web'])
        for cache in (decisions, maps):
            self.assertEqual(batch_selection.determine_many(self.root, [head], cache), expected)
            self.assertEqual((cache.hits, cache.input_hits), (0, 0))

        self.write('web/Dockerfile', (self.root / 'web/Dockerfile').read_text() + 'RUN true\n')
        self.pin_recipes()
        for cache in (None, decisions, maps):
            with self.assertRaises(SelectionIntegrityError):
                batch_selection.determine_many(self.root, [head], cache)

    def test_source_without_manifest_binds_the_exact_current_fallback(self):
        (self.root / '.github/scripts/deployables.json').unlink()
        head = self.commit()
        self.pin_recipes()
        cache = SelectionCache(self.root / 'cache', 'e' * 64)
        batch_selection.determine_many(self.root, [head], cache)
        self.manifest['cma-web']['build_args']['SYNTHETIC'] = 'changed'
        self.pin_recipes()
        actual = batch_selection.determine_many(self.root, [head], cache)
        self.assertEqual(cache.hits, 0)
        self.assertEqual(cache.misses, 2)
        self.assertEqual(cache.input_hits, 0)
        self.assertEqual(actual, batch_selection.determine_many(self.root, [head]))

    def test_corrupt_previous_inputs_are_recomputed(self):
        cache = SelectionCache(self.root / 'cache', 'e' * 64)
        batch_selection.determine_many(self.root, self.revisions[:2], cache)
        path = cache.directory / ('inputs-' + self.second + '.json')
        path.write_text('invalid')
        with patch.object(batch_selection, 'summarize', wraps=selection.summarize) as summarize:
            actual = batch_selection.determine_many(self.root, self.revisions, cache)
        self.assertEqual(summarize.call_count, 2)
        self.assertEqual(actual, batch_selection.determine_many(self.root, self.revisions))

    def test_cached_inputs_still_validate_workspace_and_recipe_integrity(self):
        cache = SelectionCache(self.root / 'cache', 'e' * 64)
        batch_selection.determine_many(self.root, self.revisions[:2], cache)
        with patch.object(selection, 'workspace', side_effect=ValueError('invalid ownership')):
            with self.assertRaises(ValueError):
                batch_selection.determine_many(self.root, self.revisions, cache)
        with patch.object(batch_selection, 'validate_recipe_sources', side_effect=ValueError('invalid recipe')):
            with self.assertRaises(ValueError):
                batch_selection.determine_many(self.root, self.revisions, cache)


if __name__ == '__main__':
    unittest.main()
