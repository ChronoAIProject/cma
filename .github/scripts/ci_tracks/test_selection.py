from pathlib import Path
import subprocess
import tempfile
import unittest

import selection
import test_batch_selection

from image_inputs import changed_images
from policy import SHARED, Workspace
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

    def test_merge_group_compares_its_entry_against_the_queue_parent(self):
        # Entry A is queued ahead of B; B's group commit is built on A's.
        for name in ('first', 'second'):
            self.git('checkout', '-qb', name, 'develop')
            (self.root / name).write_text(name)
            self.commit()
        self.git('checkout', '-qb', 'queue-a', 'develop')
        self.git('merge', '--no-ff', '-qm', 'Merge pull request #1', 'first')
        parent = self.git('rev-parse', 'HEAD')
        self.git('merge', '--no-ff', '-qm', 'Merge pull request #2', 'second')
        group = self.git('rev-parse', 'HEAD')
        event = {'merge_group': {'base_sha': parent, 'head_sha': group}}
        self.assertEqual(comparison(self.root, event, 'merge_group'), (parent, group))
        self.assertEqual(changed_paths(self.root, parent, group), ['second'])
        # A parent that is not an ancestor still compares from the merge base.
        self.git('checkout', '-q', 'develop')
        (self.root / 'unrelated').write_text('develop progressed')
        advanced = self.commit()
        event = {'merge_group': {'base_sha': advanced, 'head_sha': group}}
        self.assertEqual(comparison(self.root, event, 'merge_group'), (self.base, group))
        for broken in ({'merge_group': {'base_sha': parent}}, {'merge_group': {'base_sha': 'bad', 'head_sha': group}},
                       {'pull_request': {'base': {'sha': parent}, 'head': {'sha': group}}}):
            with self.subTest(event=broken), self.assertRaises((ValueError, KeyError)):
                comparison(self.root, broken, 'merge_group')

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

    def test_workflow_content_changes_do_not_select_images(self):
        import shutil
        workflow = '.github/workflows/ci.yml'
        self.write(workflow, 'on: pull_request\njobs:\n  image:\n    steps:\n      - uses: docker/build-push-action@v6\n        with:\n          file: web/Dockerfile\n          build-args: FOO=one\n')
        with tempfile.TemporaryDirectory() as directory:
            old = Path(directory) / 'old'
            shutil.copytree(self.root, old)
            snapshots = [(old, Workspace(metadata(old), old), self.manifest), (self.root, self.workspace, self.manifest)]
            self.write(workflow, (self.root / workflow).read_text().replace('on: pull_request', 'on: push'))
            self.assertEqual(changed_images([workflow], snapshots), [])
            self.write(workflow, (self.root / workflow).read_text().replace('FOO=one', 'FOO=two'))
            self.assertEqual(changed_images([workflow], snapshots), [])


class CIEventSelectionTests(unittest.TestCase):
    setUp = test_batch_selection.BatchTests.setUp
    write = test_batch_selection.BatchTests.write
    pin_recipes = test_batch_selection.BatchTests.pin_recipes
    git = test_batch_selection.BatchTests.git
    commit = test_batch_selection.BatchTests.commit

    def test_ci_full_events_keep_cd_and_develop_pr_selective(self):
        before = self.third
        self.write('api/crates/cmaeg/src/lib.rs', 'pub fn changed() {}')
        release_base = self.commit()
        self.write('web/src/main.tsx', 'export const app = 2;')
        head = self.commit()
        repository = 'example/repository'
        push = {'repository': {'full_name': repository}, 'ref': 'refs/heads/develop',
                'before': before, 'after': head, 'deleted': False}
        pull = {'number': 7, 'repository': {'full_name': repository}, 'pull_request': {
            'base': {'ref': 'develop', 'sha': release_base, 'repo': {'full_name': repository}},
            'head': {'ref': 'feature', 'sha': head, 'repo': {'full_name': repository}}}}
        cd = selection.determine(self.root, push, 'push')
        selective = selection.determine_ci(self.root, pull, 'pull_request', repository)
        self.assertEqual(selective, selection.determine(self.root, pull, 'pull_request'))
        self.assertFalse(selective['am'])
        self.assertEqual(cd['deployables'], ['cma-web'])
        self.assertFalse(cd['eg'])
        pull['pull_request']['base']['ref'] = 'main'
        pull['pull_request']['head']['ref'] = 'develop'
        for event, name in [(push, 'push'), (pull, 'pull_request')]:
            with self.subTest(event=name):
                result = selection.determine_ci(self.root, event, name, repository)
                baseline = selection.determine(self.root, event, name)
                self.assertEqual(result['deployables'], baseline['deployables'])
                self.assertEqual(result['test_tracks'], dict(repo=True, am=True, eg=True, web=True))
                self.assertEqual(result['head'], head)
                for track in ('am', 'eg'):
                    self.assertEqual(result[track + '_packages'], sorted(Workspace(metadata(self.root), self.root).owned[track] | SHARED))
        self.assertEqual(selection.determine(self.root, push, 'push'), cd)
        # A develop queue entry selects exactly like its PR: no full-CI coverage.
        queue = {'action': 'checks_requested', 'repository': {'full_name': repository}, 'merge_group': {
            'base_sha': release_base, 'head_sha': head, 'base_ref': 'refs/heads/develop',
            'head_ref': 'refs/heads/gh-readonly-queue/develop/pr-7-' + release_base}}
        pull['pull_request']['base']['ref'] = 'develop'
        pull['pull_request']['head']['ref'] = 'feature'
        queued = selection.determine_ci(self.root, queue, 'merge_group', repository)
        self.assertEqual(queued, selection.determine_ci(self.root, pull, 'pull_request', repository))
        self.assertFalse(queued['am'])
        self.assertEqual((queued['base'], queued['head']), (release_base, head))


if __name__ == '__main__':
    unittest.main()
