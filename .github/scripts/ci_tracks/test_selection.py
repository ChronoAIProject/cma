from pathlib import Path
import subprocess
import tempfile
import unittest

from image_inputs import changed_images
from policy import Workspace
from selection import changed_paths, comparison, snapshot
import test_images
from test_policy import metadata


class GitSelectionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.git('init', '-q', '-b', 'develop')
        self.git('config', 'user.name', 'Synthetic Test')
        self.git('config', 'user.email', 'test@example.invalid')
        (self.root / 'old file').write_text('same data\n')
        self.base = self.commit()

    def git(self, *args):
        return subprocess.check_output(['git', *args], cwd=self.root, stderr=subprocess.PIPE, text=True).strip()

    def commit(self):
        self.git('add', '.')
        self.git('commit', '-qm', 'Synthetic comparison')
        return self.git('rev-parse', 'HEAD')

    def test_renames_deletes_and_spaces(self):
        (self.root / 'old file').rename(self.root / 'new file')
        head = self.commit()
        self.assertEqual(changed_paths(self.root, self.base, head), ['new file', 'old file'])
        (self.root / 'new file').unlink()
        removed = self.commit()
        self.assertEqual(changed_paths(self.root, head, removed), ['new file'])
        with snapshot(self.root, self.base) as tree:
            self.assertTrue((tree / 'old file').exists())
        self.assertFalse(tree.exists())

    def test_pr_merge_base_and_cd_first_parent(self):
        self.git('checkout', '-qb', 'feature')
        (self.root / 'feature').write_text('change')
        head = self.commit()
        self.git('checkout', 'develop')
        (self.root / 'unrelated').write_text('base progressed')
        advanced = self.commit()
        base, compared = comparison(self.root, {'pull_request': {'base': {'sha': advanced}, 'head': {'sha': head}}}, 'pull_request')
        self.assertEqual((base, compared), (self.base, head))
        self.git('merge', '--no-ff', '-qm', 'Synthetic merge', 'feature')
        merge = self.git('rev-parse', 'HEAD')
        self.assertEqual(comparison(self.root, {'after': merge, 'before': self.base}, 'push'), (advanced, merge))

    def test_bad_or_unavailable_refs_fail(self):
        for event, name in [({}, 'pull_request'), ({'after': '--help'}, 'push'), ({}, 'unknown')]:
            with self.assertRaises((ValueError, KeyError)):
                comparison(self.root, event, name)


class SnapshotTests(unittest.TestCase):
    setUp = test_images.ImageTests.setUp
    write = test_images.ImageTests.write
    selected = test_images.ImageTests.selected
    pin_recipes = test_images.ImageTests.pin_recipes
    # Build the actual production-input fixtures once per test; use independent
    # old/new trees so deletes and graph-edge removal cannot hide a consumer.
    def test_removed_input_and_dependency_use_old_tree(self):
        import shutil
        with tempfile.TemporaryDirectory() as directory:
            old = Path(directory) / 'old'
            shutil.copytree(self.root, old)
            graph = Workspace(metadata(old), old)
            self.write('api/crates/cmaeg/tests/old.bin', 'fixture')
            old_fixture = old / 'api/crates/cmaeg/tests/old.bin'
            old_fixture.parent.mkdir(parents=True)
            old_fixture.write_text('fixture')
            old_recipe = old / 'api/crates/cmaeg/Dockerfile'
            old_recipe.write_text(old_recipe.read_text().replace('FROM base AS runtime', 'COPY api/crates/cmaeg/tests/old.bin /old.bin\nFROM base AS runtime'))
            (self.root / 'api/crates/cmaeg/tests/old.bin').unlink()
            import hashlib, json
            old_manifest = json.loads((old / '.github/scripts/deployables.json').read_text())
            old_manifest['cma-event-gateway']['dockerfile_sha256'] = hashlib.sha256(old_recipe.read_bytes()).hexdigest()
            (old / '.github/scripts/deployables.json').write_text(json.dumps(old_manifest))
            current = metadata(self.root)
            next(p for p in current['packages'] if p['name'] == 'cma')['dependencies'] = []
            snapshots = [(old, graph, self.manifest), (self.root, Workspace(current, self.root), self.manifest)]
            self.assertIn('cma-event-gateway', changed_images(['api/crates/cmaeg/tests/old.bin'], snapshots))
            self.assertIn('cma-agent-manager', changed_images(['api/crates/cma-service-auth/src/lib.rs'], snapshots))

    def test_build_recipe_change_but_not_schedule_selects_images(self):
        import shutil
        workflow = '.github/workflows/ci.yml'
        self.write(workflow, 'on: pull_request\njobs:\n  image:\n    steps:\n      - uses: docker/build-push-action@v6\n        with:\n          build-args: FOO=one\n')
        with tempfile.TemporaryDirectory() as directory:
            old = Path(directory) / 'old'
            shutil.copytree(self.root, old)
            snapshots = [(old, Workspace(metadata(old), old), self.manifest), (self.root, self.workspace, self.manifest)]
            self.write(workflow, (self.root / workflow).read_text().replace('on: pull_request', 'on: push'))
            self.assertEqual(changed_images([workflow], snapshots), [])
            self.write(workflow, (self.root / workflow).read_text().replace('FOO=one', 'FOO=two'))
            self.assertEqual(changed_images([workflow], snapshots), sorted(self.manifest))


if __name__ == '__main__':
    unittest.main()
