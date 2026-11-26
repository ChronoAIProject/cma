import copy
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from image_inputs import changed_images
from rust_inputs import cached_declarations, source_edges
import test_images
from version_label import current_pull, selected_upgrade
from web_inputs import public_directory


class SourceInputRegressions(unittest.TestCase):
    setUp = test_images.ImageTests.setUp
    write = test_images.ImageTests.write
    selected = test_images.ImageTests.selected
    pin_recipes = test_images.ImageTests.pin_recipes

    def test_r1_comments_and_strings_cannot_gate_live_module(self):
        examples = [
            '// #[cfg(test)]\nmod live;',
            '/* #[cfg(test)] */\nmod live;',
            '/* nested /* #[cfg(test)] */ comment */\nmod live;',
            'const TEXT: &str = r###"#[cfg(test)] mod live;"###;\nmod live;',
            'const TEXT: &str = "#[cfg(test)] mod live;";\nmod live;',
        ]
        self.write('api/crates/cmaeg/src/live.rs', 'pub fn shipped() {}')
        for source in examples:
            self.write('api/crates/cmaeg/src/lib.rs', source)
            with self.subTest(source=source):
                self.assertEqual(self.selected('api/crates/cmaeg/src/live.rs'), ['cma-event-gateway'])
        self.write('api/crates/cmaeg/src/lib.rs', '#[cfg(test)]\n// keep the real attribute\nmod live;')
        self.assertEqual(self.selected('api/crates/cmaeg/src/live.rs'), [])

    def test_r1_ambiguous_rust_is_retained(self):
        self.write('api/crates/cmaeg/src/live.rs', 'pub fn shipped() {}')
        for source in [
            '#[cfg_attr(test, path="else.rs")] mod live;',
            '#[cfg_attr(test, allow(dead_code), path="else.rs")] mod live;',
            '#[cfg_attr(test, external::allow(dead_code))] mod live;',
            '#[cfg(test)] mod live;\nthis is invalid rust',
        ]:
            self.write('api/crates/cmaeg/src/lib.rs', source)
            self.assertEqual(self.selected('api/crates/cmaeg/src/live.rs'), ['cma-event-gateway'])

    def test_conditional_builtin_lint_preserves_test_and_production_edges(self):
        live = 'api/crates/cmaeg/src/live.rs'
        checks = 'api/crates/cmaeg/src/checks.rs'
        self.write(live, 'pub fn shipped() {}')
        self.write(checks, 'pub fn check() {}')
        for lint in ('allow', 'expect', 'warn', 'deny', 'forbid'):
            with self.subTest(lint=lint):
                attribute = f'#[cfg_attr(not(test), {lint}(dead_code, reason = "example"),)]'
                self.write('api/crates/cmaeg/src/lib.rs',
                           f'{attribute}\nmod live;\n#[cfg(test)]\n{attribute}\nmod checks;')
                self.assertEqual(self.selected(live), ['cma-event-gateway'])
                self.assertEqual(self.selected(checks), [])

    def test_only_a_real_include_macro_makes_module_edges_uncertain(self):
        self.write('api/crates/cmaeg/src/checks.rs', 'pub fn check() {}')
        source = 'api/crates/cmaeg/src/lib.rs'
        checks = 'api/crates/cmaeg/src/checks.rs'
        self.write(source, '#[cfg(test)] mod checks; // include!("extra.rs")')
        self.assertEqual(self.selected(checks), [])
        self.write(source, '#[cfg(test)] mod checks; fn f() { let _ = include!("extra.rs"); }')
        self.assertEqual(self.selected(checks), ['cma-event-gateway'])

    def test_cached_rust_declarations_resolve_files_in_each_snapshot(self):
        with tempfile.TemporaryDirectory() as directory:
            older, newer = (Path(directory) / name for name in ('older', 'newer'))
            for snapshot in (older, newer):
                snapshot.mkdir()
                (snapshot / 'lib.rs').write_text('#[cfg(test)] mod checks;')
            (older / 'checks.rs').touch()
            (newer / 'checks').mkdir()
            alternate = newer / 'checks' / 'mod.rs'
            alternate.touch()
            cached_declarations.cache_clear()
            self.addCleanup(cached_declarations.cache_clear)
            self.assertEqual(source_edges(older / 'lib.rs'), [(older / 'checks.rs', True)])
            self.assertEqual(source_edges(newer / 'lib.rs'), [(alternate, True)])
            self.assertEqual(cached_declarations.cache_info().hits, 1)
            (newer / 'checks.rs').touch()
            self.assertIsNone(source_edges(newer / 'lib.rs'))
            (newer / 'checks.rs').unlink()
            (newer / 'lib.rs').write_text('mod checks;')
            self.assertEqual(source_edges(newer / 'lib.rs'), [(alternate, False)])

    def test_transitive_unicode_stems_survive_wide_text_and_decode_replacement(self):
        first = 'web/src/顔.test.ts'
        nested = 'web/src/é.test.ts'
        self.write('web/src/main.tsx', '// 🎈 顔.test')
        self.write(first, '// é.test')
        self.write(nested, '')
        with (self.root / first).open('ab') as stream:
            stream.write(bytes([255]))
        self.assertEqual(self.selected(nested), ['cma-web'])
        self.write(first, '// unrelated')
        self.assertEqual(self.selected(nested), [])

    def test_r2_public_assets_ship_without_source_references(self):
        self.write('web/vite.config.ts', 'export default {plugins: []};')
        for asset in ['web/public/release.test.json', 'web/public/tests/data.json', 'web/public/test/hidden.test.js']:
            self.write(asset, '{}')
            self.assertEqual(self.selected(asset), ['cma-web'])
        self.assertEqual(self.selected('web/src/main.test.tsx'), [])

    def test_r2_custom_public_dir_and_css_html_assets(self):
        self.write('web/vite.config.ts', 'export default defineConfig({publicDir: "src/test/assets"});')
        self.write('web/src/test/assets/release.test.json', '{}')
        self.assertEqual(self.selected('web/src/test/assets/release.test.json'), ['cma-web'])
        self.write('web/src/test/styles.test.css', 'body {}')
        self.write('web/src/test/referenced.test.json', '{}')
        self.write('web/index.html', '<link href="/src/test/styles.test.css"><a href="/src/test/referenced.test.json">data</a>')
        self.assertEqual(self.selected('web/src/test/styles.test.css', 'web/src/test/referenced.test.json'), ['cma-web'])

    def test_r2_dynamic_public_dir_fails_closed(self):
        self.write('web/vite.config.ts', 'export default {publicDir: process.env.ASSET_DIRECTORY};')
        with self.assertRaises(ValueError):
            self.selected('web/src/main.test.tsx')
        self.write('web/vite.config.ts', '// publicDir: "elsewhere"\nexport default {publicDir: false};')
        self.assertIsNone(public_directory(self.root))

    def test_r4_manifest_argument_change_selects_only_its_image(self):
        import shutil
        from test_policy import metadata
        from policy import Workspace
        for name in ['cma-web', 'cma-runtime-omarchy']:
            with tempfile.TemporaryDirectory() as directory:
                old = Path(directory) / 'old'
                shutil.copytree(self.root, old)
                self.manifest[name]['build_args']['FEATURE'] = 'two'
                self.pin_recipes()
                snapshots = [(old, Workspace(metadata(old), old), self.manifest), (self.root, self.workspace, self.manifest)]
                self.assertEqual(changed_images(['.github/scripts/deployables.json'], snapshots), [name])

    def test_r5_runtime_matrices_select_repo_only_but_require_a_label(self):
        from selection import analyze
        for workflow, expression in [
            ('.github/workflows/redeploy-staging.yml', '${{ fromJSON(needs.resolve.outputs.matrix) }}'),
            ('.github/workflows/staging-cd.yml', '${{ fromJSON(needs.allocate.outputs.matrix) }}'),
        ]:
            self.write(workflow, f"on: push\njobs:\n  build:\n    strategy:\n      matrix: {expression}\n    steps:\n      - uses: docker/build-push-action@v6\n        with:\n          file: ${{{{ matrix.dockerfile }}}}\n")
            snapshot = (self.root, self.workspace, self.manifest)
            result = analyze([workflow], [snapshot, snapshot])
            self.assertEqual(result['test_tracks'], {'repo': True, 'am': False, 'eg': False, 'web': False})
            self.assertEqual(result['deployables'], [])
            with self.assertRaises(ValueError):
                selected_upgrade({'pull_request': {'labels': []}}, result['deployables'])
            self.assertEqual(selected_upgrade({'pull_request': {'labels': [{'name': 'version-upgrade:patch'}]}}, result['deployables']), 'patch')
            self.assertEqual(result['deployables'], [])



class LiveLabelRegressions(unittest.TestCase):
    def setUp(self):
        self.repository = 'example/repository'
        self.pull = {'number': 7, 'state': 'open', 'labels': [],
                     'base': {'ref': 'develop', 'sha': 'a' * 40, 'repo': {'full_name': self.repository}},
                     'head': {'ref': 'feature', 'sha': 'b' * 40, 'repo': {'full_name': self.repository}}}
        self.event = {'number': 7, 'repository': {'full_name': self.repository}, 'pull_request': copy.deepcopy(self.pull)}

    def evaluate(self, labels, images):
        current = copy.deepcopy(self.pull)
        current['labels'] = [{'name': label} for label in labels]
        run = subprocess.CompletedProcess([], 0, json.dumps(current), '')
        with patch('version_label.subprocess.run', return_value=run):
            fetched = current_pull(self.event, self.repository)
        return selected_upgrade({'pull_request': fetched}, images)

    def test_r3_historical_label_is_never_authority(self):
        for images in [[], ['cma-web']]:
            self.event['pull_request']['labels'] = [{'name': 'version-upgrade:patch'}]
            for labels in [[], ['version-upgrade:patch', 'version-upgrade:major'],
                           ['version-upgrade:patch', 'version-upgrade:other']]:
                with self.subTest(images=images, labels=labels), self.assertRaises(ValueError):
                    self.evaluate(labels, images)
            self.event['pull_request']['labels'] = []
            self.assertEqual(self.evaluate(['version-upgrade:minor'], images), 'minor')

    def test_r3_stale_identity_or_api_error_fails(self):
        for side, key in [('head', 'sha'), ('head', 'ref'), ('base', 'sha'), ('base', 'ref')]:
            current = copy.deepcopy(self.pull)
            current[side][key] = 'changed'
            with patch('version_label.subprocess.run', return_value=subprocess.CompletedProcess([], 0, json.dumps(current), '')), self.assertRaises(ValueError):
                current_pull(self.event, self.repository)
        with patch('version_label.subprocess.run', side_effect=subprocess.CalledProcessError(1, ['gh'])), self.assertRaises(subprocess.CalledProcessError):
            current_pull(self.event, self.repository)


if __name__ == '__main__':
    unittest.main()
