import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from docker_inputs import ignored, stage_sources
from image_inputs import changed_images, image_inputs, recipes
from policy import Workspace
from rust_build_inputs import BuildReview
from test_policy import metadata

MANIFEST = Path(__file__).resolve().parents[1] / 'deployables.json'


class ImageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.manifest = json.loads(MANIFEST.read_text())
        for package in metadata(self.root)['packages']:
            directory = Path(package['manifest_path']).parent.relative_to(self.root).as_posix()
            self.write(directory + '/Cargo.toml', '[package]\n')
            self.write(directory + '/src/lib.rs', 'pub fn main() {}\n')
        for recipe in self.manifest.values():
            broad = 'api' if recipe['cargo_root'] else 'web'
            self.write(recipe['dockerfile'], f'FROM base AS build\nCOPY {broad}/ /build/\nFROM base AS runtime\nCOPY --from=build /build/out /out\nFROM runtime AS omarchy\nFROM runtime AS default-runtime\n')
            if recipe['cargo_root'] == 'cma-image-manager':
                # An unnamed final stage is also a valid recipe target.
                self.write(recipe['dockerfile'], 'FROM base AS build\nCOPY api/ /build/\nFROM base\nCOPY --from=build /build/out /out\n')
        self.write('api/Cargo.lock', '')
        self.write('web/src/main.tsx', 'export const app = 1;')
        self.write('web/src/main.test.tsx', 'test();')
        self.write('web/package-lock.json', '{}')
        self.write('.dockerignore', 'api/target/\n')
        self.write('.github/scripts/deployables.json', json.dumps(self.manifest))
        self.workspace = Workspace(metadata(self.root), self.root)
        self.pin_recipes()

    def write(self, path, text):
        file = self.root / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(text)

    def selected(self, *paths):
        self.pin_recipes()
        snapshot = (self.root, self.workspace, self.manifest)
        return changed_images(paths, [snapshot, snapshot])

    def pin_recipes(self):
        for recipe in self.manifest.values():
            file = self.root / recipe['dockerfile']
            if file.exists():
                recipe['dockerfile_sha256'] = hashlib.sha256(file.read_bytes()).hexdigest()
        self.write('.github/scripts/deployables.json', json.dumps(self.manifest))

    def test_ci_only_and_non_shipped_docs_do_not_publish(self):
        self.write('.github/workflows/ci.yml', 'name: CI\njobs: {}\n')
        self.write('README.md', 'public overview')
        self.write('api/deny.toml', '[advisories]')
        self.assertEqual(self.selected('.github/workflows/ci.yml', '.github/scripts/ci_tracks/gate.py', 'README.md', 'api/deny.toml'), [])

    def test_production_reverse_dependencies_and_dev_exclusion(self):
        self.assertEqual(self.selected('api/crates/cma-service-auth/src/lib.rs'), ['cma-agent-manager', 'cma-event-gateway'])
        self.assertEqual(self.selected('api/crates/cmaeg/src/lib.rs'), ['cma-event-gateway'])
        self.assertEqual(self.selected('api/crates/layer2-core/src/lib.rs'), ['cma-agent-manager'])

    def test_test_only_files_and_embedded_exception(self):
        self.write('api/crates/cmaeg/tests/example.rs', 'test')
        self.write('api/crates/cmaeg/README.md', 'text')
        self.assertEqual(self.selected('api/crates/cmaeg/tests/example.rs', 'web/src/main.test.tsx', 'api/crates/cmaeg/README.md'), [])
        self.write('api/crates/cmaeg/src/lib.rs', 'const DOC: &str = include_str!("../README.md");')
        self.assertEqual(self.selected('api/crates/cmaeg/README.md'), ['cma-event-gateway'])

    def test_manifest_relative_include_binds_one_cross_package_input(self):
        package = 'api/crates/cmaeg'
        other = 'api/crates/layer4-runtime'
        self.write(f'{other}/docs/recognized.txt', 'included bytes')
        self.write(f'{other}/docs/unrelated.txt', 'unrelated bytes')
        self.write(f'{package}/src/lib.rs', '''
            fn doc() -> Vec<&'static str> {
                vec![include_str! /* keep the macro token boundary */
                    (concat!(env!("CARGO_MANIFEST_DIR"), "/../layer4-runtime/docs/recognized.txt",))]
            }
        ''')
        self.assertEqual(self.selected(f'{other}/docs/recognized.txt'), ['cma-event-gateway'])
        self.assertEqual(self.selected(f'{other}/docs/unrelated.txt'), [])
        self.write(f'{package}/src/lib.rs', '''
            const DOC: &str = include_str!(concat!(env!("CMA_DYNAMIC_DIRECTORY"), "/recognized.txt"));
        ''')
        self.assertEqual(self.selected(f'{other}/docs/unrelated.txt'), ['cma-event-gateway'])

    def test_item_include_is_modeled_only_inside_the_scanned_sources(self):
        package = 'api/crates/cmaeg'
        self.write(f'{package}/README.md', 'included text')
        self.write(f'{package}/src/lib.rs', 'include!("included.rs");')
        self.write(f'{package}/src/included.rs', 'const DOC: &str = include_str!("../README.md");')
        self.assertEqual(self.selected(f'{package}/README.md'), ['cma-event-gateway'])
        self.assertEqual(self.selected('api/crates/cma/src/lib.rs'), ['cma-agent-manager'])
        self.write(f'{package}/docs/external.rs', 'const DOC: &str = "external Rust";')
        self.write(f'{package}/src/lib.rs', 'include!("../docs/external.rs");')
        self.assertEqual(self.selected('api/crates/cma/src/lib.rs'), ['cma-agent-manager', 'cma-event-gateway'])

    def test_cfg_test_graph_excludes_only_proven_test_modules(self):
        self.write('api/crates/cmaeg/src/lib.rs', '#[cfg(test)]\nmod checks;\n')
        self.write('api/crates/cmaeg/src/checks.rs', 'mod nested;\n')
        self.write('api/crates/cmaeg/src/checks/nested.rs', 'const DOC: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md"));')
        self.assertEqual(self.selected('api/crates/cmaeg/src/checks/nested.rs'), [])
        self.write('api/crates/cmaeg/src/lib.rs', 'mod checks;\n')
        self.assertEqual(self.selected('api/crates/cmaeg/src/checks/nested.rs'), ['cma-event-gateway'])

    def test_direct_copy_of_fixture_and_docs_always_counts(self):
        self.write('api/crates/cmaeg/tests/fixtures/a.bin', 'image fixture')
        self.write('api/crates/cmaeg/assets/docs/public/guide.md', 'shipped guide')
        with (self.root / 'api/crates/cmaeg/Dockerfile').open('a') as file:
            file.write('COPY api/crates/cmaeg/tests/fixtures/ /fixtures/\n')
        self.manifest['cma-event-gateway']['target'] = 'default-runtime'
        self.write('.github/scripts/deployables.json', json.dumps(self.manifest))
        with (self.root / 'web/Dockerfile').open('a') as file:
            file.write('COPY api/crates/cmaeg/assets/docs/ /docs/\n')
        self.manifest['cma-web']['target'] = 'default-runtime'
        self.write('.github/scripts/deployables.json', json.dumps(self.manifest))
        self.assertEqual(self.selected('api/crates/cmaeg/tests/fixtures/a.bin'), ['cma-event-gateway'])
        self.assertEqual(self.selected('api/crates/cmaeg/assets/docs/public/guide.md'), ['cma-event-gateway', 'cma-web'])

    def test_local_mount_and_ci_patch_inputs(self):
        self.write('.github/scripts/patch.diff', 'patch')
        path = self.root / 'api/crates/layer4-runtime/Dockerfile'
        path.write_text(path.read_text().replace('FROM base AS runtime', 'RUN --mount=type=bind,source=.github/scripts/patch.diff,target=/patch apply\nFROM base AS runtime'))
        self.assertEqual(self.selected('.github/scripts/patch.diff'), ['cma-runtime', 'cma-runtime-omarchy'])

    def test_build_scripts_conservatively_include_package_files(self):
        self.write('api/crates/cmaeg/build.rs', 'fn main() {}')
        self.write('api/crates/cmaeg/docs/generator.txt', 'generated')
        self.assertEqual(self.selected('api/crates/cmaeg/docs/generator.txt'), ['cma-event-gateway'])

    def test_reviewed_generator_limits_inputs_to_its_dependency_closure(self):
        package = 'api/crates/layer3-adapter'
        foreign = 'api/crates/cmaeg/src/lib.rs'
        data = 'docs/declaration.txt'
        files = {
            'build.rs': 'fn main() {}',
            'build/helper.rs': 'fn encode() {}',
            'src/lib.rs': 'include!(concat!(env!("OUT_DIR"), "/generated.rs"));',
        }
        for path, text in files.items():
            self.write(f'{package}/{path}', text)
        self.write(f'{package}/{data}', 'input')
        model = metadata(self.root)
        runtime = next(item for item in model['packages'] if item['name'] == 'layer4-runtime')
        runtime['dependencies'].append({'path': str(self.root / package), 'kind': None})
        self.workspace = Workspace(model, self.root)
        reviewed = BuildReview('build.rs', {name: hashlib.sha256(text.encode()).hexdigest()
                                           for name, text in files.items()},
                               (data,), frozenset({'src/lib.rs'}))
        affected = ['cma-agent-manager', 'cma-runtime', 'cma-runtime-omarchy']
        with patch('rust_build_inputs.REVIEWED', {package: reviewed}):
            self.assertEqual(self.selected(f'{package}/{data}'), affected)
            self.assertEqual(self.selected(f'{package}/build/helper.rs'), affected)
            self.assertEqual(self.selected(foreign), ['cma-event-gateway'])
            for path, text in (
                ('build/helper.rs', 'fn encode_changed() {}'),
                ('src/lib.rs', 'include!(concat!(env!("OUT_DIR"), "/other.rs"));'),
            ):
                with self.subTest(path=path):
                    self.write(f'{package}/{path}', text)
                    self.assertEqual(self.selected(foreign), sorted(affected + ['cma-event-gateway']))
                    self.write(f'{package}/{path}', files[path])

    def test_custom_build_target_outside_the_conventional_path_stays_conservative(self):
        package = 'api/crates/cmaeg'
        self.write(f'{package}/scripts/build.rs', 'fn main() {}')
        model = metadata(self.root)
        gateway = next(item for item in model['packages'] if item['name'] == 'cmaeg')
        gateway['targets'] = [{'kind': ['custom-build'], 'src_path': str(self.root / package / 'scripts/build.rs')}]
        self.workspace = Workspace(model, self.root)
        self.assertEqual(self.selected('api/crates/layer4-runtime/src/lib.rs'),
                         ['cma-event-gateway', 'cma-runtime', 'cma-runtime-omarchy'])

    def test_metadata_inputs_select_rust_only(self):
        self.assertEqual(self.selected('api/Cargo.lock'), sorted(set(self.manifest) - {'cma-web'}))

    def test_missing_recipe_and_unsupported_sources_fail(self):
        file = self.root / 'web/Dockerfile'
        file.unlink()
        with self.assertRaises(ValueError):
            self.selected('web/src/main.tsx')
        self.write('web/Dockerfile', 'FROM base\nCOPY ${UNRESOLVED}/ /out\n')
        with self.assertRaises(ValueError):
            image_inputs(self.root, self.workspace)
        (self.root / '.github/scripts/deployables.json').unlink()
        with self.assertRaises(ValueError):
            recipes(self.root, {})

    def test_changed_dockerfile_requires_model_review(self):
        self.write('web/Dockerfile', (self.root / 'web/Dockerfile').read_text() + 'RUN true\n')
        with self.assertRaisesRegex(ValueError, 'production-input model'):
            image_inputs(self.root, self.workspace)

    def test_build_variants_and_canonical_identity(self):
        from recipe import resolve
        self.assertEqual(resolve(self.root, 'api')['name'], 'cma-agent-manager')
        self.assertEqual(resolve(self.root, 'cmaeg')['name'], 'cma-event-gateway')
        self.assertEqual(resolve(self.root, 'runtime-omarchy')['build_args']['RUNTIME_DESKTOP'], '1')
        for name, recipe in self.manifest.items():
            self.assertIn(name, self.selected(recipe['dockerfile']))
        with self.assertRaises(ValueError):
            resolve(self.root, 'unknown')

    def test_stage_graph_json_copy_and_ignore(self):
        text = 'FROM base AS unused\nCOPY missing /unused\nFROM base AS build\nCOPY ["input", "/src"]\nFROM build AS final\n'
        self.assertEqual(stage_sources(text, 'final', {}), {'input'})
        self.assertTrue(ignored('api/target/out', 'api/target/'))
        self.assertFalse(ignored('.github/scripts/patch.diff', '.github/*\n!.github/scripts/\n.github/scripts/*\n!.github/scripts/patch.diff'))
        for text in ['FROM base\nCOPY --unknown=x source /out', 'FROM base\nCOPY --from=external /out /out', 'FROM base\nRUN --mount=type=bind,source=$INPUT,target=/in true']:
            with self.assertRaises(ValueError):
                stage_sources(text, '', {})

    def test_ignore_decisions_use_the_exact_ordered_rule_text(self):
        entry = 'api/crates/sample/src/generated.rs'
        exclude = '**/generated.rs'
        include = '!api/crates/sample/src/generated.rs'
        self.assertTrue(ignored(entry, exclude))
        self.assertFalse(ignored(entry, f'{exclude}\n{include}'))
        self.assertTrue(ignored(entry, f'{include}\n{exclude}'))
        self.assertTrue(ignored(entry, exclude))


if __name__ == '__main__':
    unittest.main()
