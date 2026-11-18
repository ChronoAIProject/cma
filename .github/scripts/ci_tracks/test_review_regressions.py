import copy
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from image_inputs import changed_images
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
        for source in ['#[cfg_attr(test, path="else.rs")] mod live;', '#[cfg(test)] mod live;\nthis is invalid rust']:
            self.write('api/crates/cmaeg/src/lib.rs', source)
            self.assertEqual(self.selected('api/crates/cmaeg/src/live.rs'), ['cma-event-gateway'])

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

    def test_r5_runtime_matrices_select_repo_only_without_labels(self):
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
            self.assertEqual(selected_upgrade({'pull_request': {'labels': []}}, result['deployables']), 'not-required')



class LiveLabelRegressions(unittest.TestCase):
    def setUp(self):
        self.repository = 'example/repository'
        self.pull = {'number': 7, 'state': 'open', 'labels': [],
                     'base': {'ref': 'develop', 'sha': 'a' * 40, 'repo': {'full_name': self.repository}},
                     'head': {'ref': 'feature', 'sha': 'b' * 40, 'repo': {'full_name': self.repository}}}
        self.event = {'number': 7, 'repository': {'full_name': self.repository}, 'pull_request': copy.deepcopy(self.pull)}

    def evaluate(self, labels):
        current = copy.deepcopy(self.pull)
        current['labels'] = [{'name': label} for label in labels]
        run = subprocess.CompletedProcess([], 0, json.dumps(current), '')
        with patch('version_label.subprocess.run', return_value=run):
            fetched = current_pull(self.event, self.repository)
        return selected_upgrade({'pull_request': fetched}, ['cma-web'])

    def test_r3_historical_label_is_never_authority(self):
        self.event['pull_request']['labels'] = [{'name': 'version-upgrade:patch'}]
        for labels in [[], ['version-upgrade:patch', 'version-upgrade:major']]:
            with self.assertRaises(ValueError):
                self.evaluate(labels)
        self.event['pull_request']['labels'] = []
        self.assertEqual(self.evaluate(['version-upgrade:minor']), 'minor')

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
