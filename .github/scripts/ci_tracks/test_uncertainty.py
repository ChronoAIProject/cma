"""Uncertain input models broaden coverage; integrity failures still refuse."""

from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import batch_selection
from image_inputs import NAMES
import selection
from selection_cache import SelectionCache
import test_batch_selection
import test_retired_images
from uncertainty import SelectionIntegrityError


class UncertaintyTests(unittest.TestCase):
    setUp = test_batch_selection.BatchTests.setUp
    write = test_batch_selection.BatchTests.write
    pin_recipes = test_batch_selection.BatchTests.pin_recipes
    git = test_batch_selection.BatchTests.git
    commit = test_batch_selection.BatchTests.commit

    def assert_all(self, value, reason):
        self.assertEqual(value['deployables'], sorted(NAMES))
        self.assertEqual(value['reasons'], [reason])
        self.assertEqual(value['test_tracks'], {key: True for key in ('repo', 'am', 'eg', 'web')})
        self.assertTrue(value['am_packages'])
        self.assertTrue(value['eg_packages'])

    def spread_commit(self):
        self.write('web/vite.config.ts', 'import config from "./shared"; export default { ...config };')
        self.write('web/shared.ts', 'export default { publicDir: "assets" };')
        self.write('web/assets/new.test.json', '{}')
        return self.commit()

    def test_reviewed_vite_spread_repro_cold_warm_and_corrupt_cache(self):
        head = self.spread_commit()
        cache = SelectionCache(self.root / 'cache', 'e' * 64)
        cold = batch_selection.determine_many(self.root, [head], cache)
        self.assert_all(cold[0], 'input-analysis-uncertain')
        self.assertEqual((cold[0]['base'], cold[0]['head']), (self.third, head))
        with patch.object(batch_selection, 'summarize', side_effect=AssertionError('cache should hit')):
            self.assertEqual(batch_selection.determine_many(self.root, [head], cache), cold)
        cache.path(head).write_text('invalid cache record')
        self.assertEqual(batch_selection.determine_many(self.root, [head], cache), cold)
        pull = {'pull_request': {'base': {'sha': self.third}, 'head': {'sha': head}}}
        self.assertEqual(selection.determine(self.root, pull, 'pull_request'), cold[0])
        batch = self.root / 'batch.json'
        batch.write_text(json.dumps([head]))
        stdout = io.StringIO()
        with patch('sys.argv', ['batch', str(self.root), str(batch)]), redirect_stdout(stdout):
            self.assertEqual(batch_selection.main(), 0)
        self.assertEqual(json.loads(stdout.getvalue()), cold)

    def test_analysis_error_is_sanitized_and_does_not_abort_later_comparisons(self):
        with patch.object(selection, 'image_inputs', side_effect=ValueError('untrusted parser details')):
            values = batch_selection.determine_many(self.root, self.revisions)
        self.assertEqual([value['head'] for value in values], self.revisions)
        for value in values:
            self.assert_all(value, 'input-analysis-uncertain')
        self.assertNotIn('untrusted parser details', json.dumps(values))

    def test_historical_manifest_uncertainty_selects_only_current_images(self):
        test_retired_images.RetirementTests.legacy(self, self.root)
        self.commit()
        self.write('unmodeled/production.input', 'synthetic input')
        head = self.commit()
        for reason in ('unknown-input-path', 'input-analysis-uncertain',
                       'shallow-history', 'comparison-boundary-not-found'):
            with self.subTest(reason=reason):
                if reason == 'input-analysis-uncertain':
                    target, kwargs = 'selection.image_inputs', {'side_effect': ValueError('synthetic')}
                elif reason == 'unknown-input-path':
                    target, kwargs = 'selection.unknown_paths', {'return_value': True}
                else:
                    target, kwargs = 'selection.comparison_details', {'return_value': (self.third, head, reason)}
                with patch(target, **kwargs):
                    self.assert_all(selection.determine(self.root, {'after': head}, 'push'), reason)
                if reason in ('shallow-history', 'comparison-boundary-not-found'):
                    target = 'batch_selection.comparison_details'
                with patch(target, **kwargs):
                    self.assert_all(batch_selection.determine_many(self.root, [head])[0], reason)

    def test_unknown_path_selects_all_for_pr_and_cd(self):
        for path in ('unmodeled/production.input', 'api/crates/unmodeled/source.rs'):
            parent = self.git('rev-parse', 'HEAD')
            self.write(path, 'synthetic input')
            head = self.commit()
            result = selection.determine(self.root, {'after': head}, 'push')
            self.assert_all(result, 'unknown-input-path')
            self.assertEqual(batch_selection.determine_many(self.root, [head]), [result])
            event = {'pull_request': {'base': {'sha': parent}, 'head': {'sha': head}}}
            self.assertEqual(selection.determine(self.root, event, 'pull_request'), result)

    def test_real_shallow_clone_selects_all_using_recorded_parent_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            clone = Path(directory) / 'clone'
            subprocess.run(['git', 'clone', '--no-local', '--depth=1', '--branch=develop', str(self.root), str(clone)],
                           check=True, capture_output=True)
            value = selection.determine(clone, {'after': self.third}, 'push')
            self.assert_all(value, 'shallow-history')
            self.assertEqual((value['base'], value['head']), (self.second, self.third))
            self.assertEqual(batch_selection.determine_many(clone, [self.third]), [value])
            event = {'pull_request': {'base': {'sha': self.second}, 'head': {'sha': self.third}}}
            self.assertEqual(selection.determine(clone, event, 'pull_request'), value)
            cache = SelectionCache(Path(directory) / 'cache', 'e' * 64)
            cached = batch_selection.determine_many(self.root, [self.third], cache)[0]
            self.assertNotEqual(cached['deployables'], sorted(NAMES))
            self.assertEqual(batch_selection.determine_many(clone, [self.third], cache), [value])

    def test_missing_comparison_boundary_selects_all_for_valid_commits(self):
        tree = self.git('rev-parse', self.third + '^{tree}')
        unrelated = self.git('commit-tree', tree, '-m', 'Synthetic unrelated root')
        event = {'pull_request': {'base': {'sha': unrelated}, 'head': {'sha': self.third}}}
        value = selection.determine(self.root, event, 'pull_request')
        self.assert_all(value, 'comparison-boundary-not-found')
        self.assertEqual((value['base'], value['head']), (unrelated, self.third))
        root = selection.determine(self.root, {'after': self.base}, 'push')
        self.assert_all(root, 'comparison-boundary-not-found')
        self.assertEqual(batch_selection.determine_many(self.root, [self.base]), [root])

    def test_recipe_integrity_is_not_hidden_by_an_uncertain_web_model(self):
        self.spread_commit()
        self.write('api/crates/cma/Dockerfile', 'FROM other\n')
        head = self.commit()
        with self.assertRaises(SelectionIntegrityError):
            selection.determine(self.root, {'after': head}, 'push')
        with self.assertRaises(SelectionIntegrityError):
            batch_selection.determine_many(self.root, [head])

    def test_invalid_sha_missing_source_and_workspace_ownership_still_refuse(self):
        for head in ('--help', 'f' * 40):
            with self.assertRaises((ValueError, subprocess.CalledProcessError)):
                selection.determine(self.root, {'after': head}, 'push')
        with patch.object(selection, 'workspace', side_effect=ValueError('invalid ownership')):
            with self.assertRaises(ValueError):
                batch_selection.determine_many(self.root, [self.first])

    def test_spread_repro_runs_through_actual_batch_subprocess(self):
        members = sorted(path.parent for path in (self.root / 'api/crates').glob('*/Cargo.toml'))
        for path in members:
            (path / 'Cargo.toml').write_text('[package]\nname = "' + path.name +
                                           '"\nversion = "0.1.0"\nedition = "2024"\n')
        self.write('api/Cargo.toml', '[workspace]\nresolver = "3"\nmembers = ' +
                   json.dumps(['crates/' + path.name for path in members]) + '\n')
        subprocess.run(['cargo', 'generate-lockfile', '--offline'], cwd=self.root / 'api',
                       check=True, capture_output=True)
        self.commit()
        head = self.spread_commit()
        batch = self.root / 'batch.json'
        batch.write_text(json.dumps([head]))
        run = subprocess.run([sys.executable, '-B', str(Path(batch_selection.__file__)),
                              str(self.root), str(batch)], capture_output=True, text=True, check=True)
        rows = json.loads(run.stdout)
        self.assertEqual(len(rows), 1)
        self.assert_all(rows[0], 'input-analysis-uncertain')
        self.assertEqual(rows[0]['head'], head)


if __name__ == '__main__':
    unittest.main()
