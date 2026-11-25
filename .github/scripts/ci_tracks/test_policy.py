from pathlib import Path
import unittest

from policy import AM, EG, SHARED, Workspace, select
from rust import command, packages


def metadata(root):
    names = AM | EG | SHARED
    packages = []
    edges = {'cma': [('cma-service-auth', None), ('layer3-adapter', None), ('cmaeg', 'dev')],
             'cmaeg': [('cma-service-auth', None), ('cmaeg-core', None), ('cma-allocator-qualification', None)],
             'layer3-adapter': [('layer2-core', 'build')], 'layer2-core': [('layer1-public', None)]}
    for name in sorted(names):
        dependencies = [{'name': target, 'path': str(root / 'api/crates' / target), 'kind': kind}
                        for target, kind in edges.get(name, [])]
        packages.append({'id': name, 'name': name, 'manifest_path': str(root / 'api/crates' / name / 'Cargo.toml'), 'dependencies': dependencies})
    return {'workspace_root': str(root / 'api'), 'workspace_members': sorted(names), 'packages': packages}


class PolicyTests(unittest.TestCase):
    def setUp(self):
        self.root = Path('/synthetic')
        self.graph = Workspace(metadata(self.root), self.root)

    def test_web_and_workspace_configuration(self):
        result = select(['web/src/main.tsx'], self.graph)
        self.assertTrue(result['web'])
        self.assertFalse(result['am'] or result['eg'])
        self.assertEqual(result['am_packages'], [])
        result = select(['api/Cargo.lock'], self.graph)
        self.assertTrue(result['am'] and result['eg'])

    def test_dev_and_build_dependencies_select_tests(self):
        self.assertIn('cma', self.graph.affected('api/crates/cmaeg/src/lib.rs'))
        self.assertIn('cma', self.graph.affected('api/crates/layer2-core/src/lib.rs'))
        self.assertNotIn('cmaeg', self.graph.production['cma'])
        result = select(['api/crates/cmaeg/src/lib.rs'], self.graph)
        self.assertTrue(result['am'] and result['eg'])

    def test_shared_roots_and_full_suite(self):
        for name in SHARED:
            result = select([f'api/crates/{name}/src/lib.rs'], self.graph)
            self.assertTrue(result['am'] and result['eg'], name)
        result = select(['api/crates/cmaeg/Dockerfile'], self.graph)
        self.assertFalse(result['am'])
        self.assertEqual(set(result['eg_packages']), {'cmaeg', 'cmaeg-core', 'cma-service-auth', 'cma-allocator-qualification'})
        self.assertEqual(self.graph.owned['am'] | self.graph.owned['eg'], self.graph.names)

    def test_docs_assets_select_web_and_gateway(self):
        result = select(['api/crates/cmaeg/assets/docs/public/guide.md'], self.graph)
        self.assertTrue(result['web'] and result['eg'])

    def test_unknown_and_ci_paths_broaden_tests(self):
        for path in ['new-file', 'api/unknown/config', 'api/crates/new/src/lib.rs']:
            result = select([path], self.graph)
            self.assertTrue(all(result[key] for key in ('repo', 'am', 'eg', 'web')))

    def test_reject_invalid_metadata_and_paths(self):
        for key in ['packages', 'workspace_members']:
            bad = metadata(self.root)
            bad[key].pop()
            with self.assertRaises(ValueError):
                Workspace(bad, self.root)
        with self.assertRaises(ValueError):
            select(['../escape'], self.graph)

    def test_key_provider_contract_paths_select_am(self):
        for path in ['.github/scripts/work-recovery/provider-protocol.mjs',
                     '.github/scripts/work-recovery/fixtures/mint-expiry.json',
                     '.github/scripts/key-provider-contracts.py',
                     '.github/scripts/key-provider-contracts.test.mjs',
                     '.github/scripts/mongo-contract-exceptions.json']:
            with self.subTest(path=path):
                self.assertTrue(select([path], self.graph)['am'])

    def test_new_dependency_inherits_owner(self):
        data = metadata(self.root)
        data['packages'].append({'id': 'new', 'name': 'new', 'manifest_path': '/synthetic/api/crates/new/Cargo.toml', 'dependencies': []})
        data['workspace_members'].append('new')
        next(p for p in data['packages'] if p['name'] == 'cma')['dependencies'].append({'path': '/synthetic/api/crates/new', 'kind': None})
        result = select(['api/crates/new/src/lib.rs'], Workspace(data, self.root))
        self.assertTrue(result['am'])
        self.assertFalse(result['eg'])
        self.assertIn('new', result['am_packages'])

    def test_journey_probe_is_owned_only_where_present(self):
        name = 'cma-journey-probe'
        self.assertNotIn(name, self.graph.owned['am'])

        data = metadata(self.root)
        directory = self.root / 'api/crates' / name
        data['packages'].append({'id': name, 'name': name, 'manifest_path': str(directory / 'Cargo.toml'),
                                 'dependencies': []})
        data['workspace_members'].append(name)
        head = Workspace(data, self.root)
        result = select([f'api/crates/{name}/src/main.rs'], head)
        self.assertTrue(result['am'])
        self.assertFalse(result['eg'])
        self.assertIn(name, result['am_packages'])
        self.assertNotIn(name, head.production['cma'])

    def test_rust_commands_preserve_lock_and_partition(self):
        for action in ['lint', 'build', 'doc']:
            argv = command(action, ['cma', 'cma-service-auth'])
            self.assertIn('--locked', argv)
            self.assertNotIn('--workspace', argv)
        argv = command('lint', ['cma'])
        self.assertIn('--all-targets', argv)
        self.assertIn('--all-features', argv)
        for shard in (1, 2, 3):
            argv = command('tests', ['cmaeg'], shard)
            self.assertIn(f'count:{shard}/3', argv)
            self.assertIn('package(=cmaeg)', argv[argv.index('-E') + 1])
        allocator = "package(=cma) and binary(=allocator_cli) and test(=actual_api_allocator_completes_without_configuration)"
        for names in (["cma"], ["*"]):
            for shard in (1, 2, 3):
                argv = command('tests', names, shard)
                selection = argv[argv.index('-E') + 1]
                self.assertEqual(selection.count(allocator), 1)
                self.assertIn("and not (binary(=media_image_peak) or binary(=turn_metrics) or (" + allocator + "))", selection)
                self.assertNotIn("not binary(=allocator_cli)", selection)
        for invalid in ['[]', 'null', '["--workspace"]', '[1]']:
            with self.assertRaises(ValueError):
                packages(invalid)
        with self.assertRaises(ValueError):
            command('tests', ['cmaeg'], 4)


if __name__ == '__main__':
    unittest.main()
