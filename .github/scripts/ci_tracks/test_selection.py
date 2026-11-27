from pathlib import Path
import os
import subprocess
import tempfile
import unittest
from unittest.mock import patch

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
        self.assertEqual(selective, dict(selection.determine(self.root, pull, 'pull_request'),
                                         approval_contract=False))
        self.assertFalse(selective['am'])
        self.assertEqual(cd['deployables'], ['cma-web'])
        self.assertFalse(cd['eg'])
        pull['pull_request']['base']['ref'] = 'main'
        pull['pull_request']['head']['ref'] = 'develop'
        candidate = dict(push, ref='refs/heads/tl/release/frozen-candidate', before='0' * 40)
        for event, name in [(push, 'push'), (candidate, 'push'), (pull, 'pull_request')]:
            with self.subTest(event=name):
                result = selection.determine_ci(self.root, event, name, repository)
                baseline = selection.determine(self.root, event, name)
                self.assertEqual(result['deployables'], baseline['deployables'])
                self.assertEqual(result['test_tracks'], dict(repo=True, am=True, eg=True, web=True))
                self.assertTrue(result['approval_contract'])
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

    def test_ci_uses_checkout_rust_packages_for_stale_pr_and_merge_queue(self):
        retired = 'cma-image-manager'
        added = 'cma-journey-probe'
        self.write(f'api/crates/{retired}/Cargo.toml', '# historical workspace member')
        old_base = self.commit()
        self.git('checkout', '-qb', 'stale-pr')
        self.write('api/crates/cma/src/lib.rs', 'pub fn changed() {}')
        pr_head = self.commit()
        self.git('checkout', '-q', 'develop')
        (self.root / 'api/crates' / retired / 'Cargo.toml').unlink()
        self.write(f'api/crates/{added}/Cargo.toml', '# current workspace member')
        queue_base = self.commit()
        self.git('merge', '--no-ff', '-qm', 'Synthetic merge checkout', 'stale-pr')
        checkout = self.git('rev-parse', 'HEAD')
        repository = 'example/repository'
        pull = {'number': 7, 'repository': {'full_name': repository}, 'pull_request': {
            'base': {'ref': 'develop', 'sha': old_base, 'repo': {'full_name': repository}},
            'head': {'ref': 'stale-pr', 'sha': pr_head, 'repo': {'full_name': repository}}}}
        queue = {'action': 'checks_requested', 'repository': {'full_name': repository},
                 'merge_group': {'base_sha': queue_base, 'head_sha': checkout,
                                 'base_ref': 'refs/heads/develop',
                                 'head_ref': 'refs/heads/gh-readonly-queue/develop/pr-7-' + queue_base}}

        def workspace(tree):
            data = metadata(tree)
            for name in (retired, added):
                manifest = tree / 'api/crates' / name / 'Cargo.toml'
                if manifest.exists():
                    data['workspace_members'].append(name)
                    data['packages'].append({'id': name, 'name': name, 'dependencies': [],
                                             'manifest_path': str(manifest)})
            return Workspace(data, tree)

        with patch.object(selection, 'workspace', side_effect=workspace):
            compared = selection.determine(self.root, pull, 'pull_request')
            self.assertIn(retired, compared['am_packages'])
            for event, name, source in [(pull, 'pull_request', pr_head), (queue, 'merge_group', checkout)]:
                with self.subTest(event=name):
                    result = selection.determine_ci(self.root, event, name, repository)
                    self.assertEqual(result['test_tracks'], dict(repo=True, am=True, eg=False, web=False))
                    self.assertEqual(result['head'], source)
                    self.assertEqual(result['base'], old_base if name == 'pull_request' else queue_base)
                    self.assertEqual(result['deployables'], compared['deployables'])
                    self.assertEqual(result['am_packages'], sorted(workspace(self.root).owned['am']))
                    self.assertEqual(result['eg_packages'], [])

        with patch.dict(os.environ, GITHUB_SHA=checkout):
            selection.validate_ci_checkout(self.root, queue, 'merge_group')
            self.git('checkout', '-q', 'stale-pr')
            with self.assertRaisesRegex(ValueError, 'CI checkout differs'):
                selection.validate_ci_checkout(self.root, queue, 'merge_group')
        with patch.dict(os.environ, GITHUB_SHA=pr_head):
            with self.assertRaisesRegex(ValueError, 'merge queue checkout differs'):
                selection.validate_ci_checkout(self.root, queue, 'merge_group')

    def test_pr_checkout_preserves_removed_paths_and_adds_new_rust_owners(self):
        checkout_marker = self.root / 'new-eg-owner'
        helper = 'cma-journey-probe'
        helper_path = f'api/crates/{helper}/src/lib.rs'
        changed = {'src': helper_path}

        def graph(tree):
            data = metadata(tree)
            if tree != self.root or changed['src'] == helper_path:
                manifest = tree / 'api/crates' / helper / 'Cargo.toml'
                data['workspace_members'].append(helper)
                data['packages'].append({'id': helper, 'name': helper, 'dependencies': [],
                                         'manifest_path': str(manifest)})
                if checkout_marker.exists() and tree == self.root:
                    eg = next(package for package in data['packages'] if package['name'] == 'cmaeg')
                    eg['dependencies'].append({'path': str(manifest.parent), 'kind': None})
            return Workspace(data, tree)

        repository = 'example/repository'
        pull = {'number': 7, 'repository': {'full_name': repository}, 'pull_request': {
            'base': {'ref': 'develop', 'sha': self.base, 'repo': {'full_name': repository}},
            'head': {'ref': 'feature', 'sha': self.third, 'repo': {'full_name': repository}}}}
        base_result = selection.select([helper_path], graph(self.root))
        base_result.update(base=self.base, head=self.third, deployables=['cma-web'])
        with patch.object(selection, 'determine', side_effect=lambda *_args, **_kw: dict(base_result)), \
                patch.object(selection, 'workspace', side_effect=graph), \
                patch.object(selection, 'changed_paths', side_effect=lambda *_args: [changed['src']]):
            checkout_marker.touch()
            result = selection.determine_ci(self.root, pull, 'pull_request', repository)
            self.assertEqual(result['test_tracks'], dict(repo=True, am=True, eg=True, web=False))
            self.assertIn(helper, result['eg_packages'])
            self.assertTrue(result['approval_contract'])
            changed['src'] = 'api/crates/cma-image-manager/src/lib.rs'
            result = selection.determine_ci(self.root, pull, 'pull_request', repository)
            self.assertEqual(result['test_tracks'], dict(repo=True, am=True, eg=False, web=False))
            self.assertEqual(result['am_packages'], sorted(graph(self.root).owned['am']))
            self.assertEqual(result['deployables'], ['cma-web'])
            base_result.update(am=True, eg=True, web=True, reasons=['unknown-input-path'])
            result = selection.determine_ci(self.root, pull, 'pull_request', repository)
            self.assertEqual(set(result['eg_packages']), graph(self.root).owned['eg'] | SHARED)
            self.assertEqual(result['deployables'], ['cma-web'])

    def test_approval_contract_web_inputs_do_not_select_rust_product_tracks(self):
        repository = 'example/repository'
        for path in [
            'web/src/api-client/agent-events-v2/index.ts',
            'web/src/api-client/tests/external-approval-wire.contract.ts',
            'web/src/agent/conversation.ts',
            'web/src/agent/transport/projection.ts',
            'web/build/identity.ts',
            'web/vite.config.ts',
            'web/package.json',
            'web/package-lock.json',
        ]:
            with self.subTest(path=path):
                base = self.git('rev-parse', 'HEAD')
                if path.endswith('.json'):
                    content = '{"name":"synthetic","private":true}'
                else:
                    content = 'export default { test: { globals: true } };'
                self.write(path, content)
                head = self.commit()
                event = {'number': 7, 'repository': {'full_name': repository}, 'pull_request': {
                    'base': {'ref': 'develop', 'sha': base, 'repo': {'full_name': repository}},
                    'head': {'ref': 'feature', 'sha': head, 'repo': {'full_name': repository}},
                }}
                result = selection.determine_ci(self.root, event, 'pull_request', repository)
                self.assertTrue(result['approval_contract'])
                self.assertEqual(result['test_tracks'], dict(repo=True, am=False, eg=False, web=True))
                self.assertEqual(result['am_packages'], [])
                self.assertEqual(result['eg_packages'], [])
                self.assertLessEqual(set(result['deployables']), {'cma-web'})
                self.assertNotIn('input-analysis-uncertain', result['reasons'])


    def test_release_base_the_policy_cannot_model_selects_every_deployable(self):
        # The previous release can predate the current ownership policy. For a
        # develop-to-main release that base is uncertainty, never a narrower
        # selection; a develop PR with the same failure still fails closed.
        from unittest import mock
        self.write('web/src/main.tsx', 'export const app = 3;')
        head = self.commit()
        repository = 'example/repository'

        def pull(base_ref, head_ref):
            return {'number': 9, 'repository': {'full_name': repository}, 'pull_request': {
                'base': {'ref': base_ref, 'sha': self.third, 'repo': {'full_name': repository}},
                'head': {'ref': head_ref, 'sha': head, 'repo': {'full_name': repository}}}}

        real = selection.summarize
        calls = []

        def summarize(tree, fallback):
            calls.append(tree)
            if len(calls) == 2:
                raise ValueError('missing or ambiguous package ownership')
            return real(tree, fallback)

        with mock.patch.object(selection, 'summarize', side_effect=summarize):
            result = selection.determine_ci(self.root, pull('main', 'develop'), 'pull_request', repository)
        self.assertEqual(len(calls), 2)
        self.assertEqual(result['deployables'], sorted(selection.NAMES))
        self.assertEqual(result['test_tracks'], dict(repo=True, am=True, eg=True, web=True))
        self.assertEqual(set(result['reasons']), {'release-base-unmodeled', 'full-ci-event'})
        self.assertEqual((result['base'], result['head']), (self.third, head))

        calls.clear()
        with mock.patch.object(selection, 'summarize', side_effect=summarize):
            with self.assertRaises(ValueError):
                selection.determine_ci(self.root, pull('develop', 'feature'), 'pull_request', repository)

        def broken_head(tree, fallback):
            raise ValueError('head ownership')

        with mock.patch.object(selection, 'summarize', side_effect=broken_head):
            with self.assertRaises(ValueError):
                selection.determine_ci(self.root, pull('main', 'develop'), 'pull_request', repository)


    def test_develop_verification_is_full_and_uses_the_exact_event_commit(self):
        from unittest import mock
        repository = 'example/repository'
        event = {'repository': {'full_name': repository}}
        environment = {'GITHUB_REF': 'refs/heads/develop', 'GITHUB_SHA': self.third}
        for name in ('schedule', 'workflow_dispatch'):
            with self.subTest(event=name), mock.patch.dict('os.environ', environment):
                result = selection.determine_ci(self.root, event, name, repository)
                self.assertEqual((result['base'], result['head']), (self.second, self.third))
                self.assertEqual(result['test_tracks'], dict(repo=True, am=True, eg=True, web=True))
            for change in ({'GITHUB_REF': 'refs/heads/main'}, {'GITHUB_SHA': '0' * 40}):
                with self.subTest(event=name, change=change), mock.patch.dict('os.environ', environment | change):
                    with self.assertRaises(ValueError):
                        selection.determine_ci(self.root, event, name, repository)

    def test_isolated_candidate_push_requires_a_live_commit_and_exact_namespace(self):
        from ci_events import full_ci_required

        repository = 'example/repository'
        push = {'repository': {'full_name': repository}, 'ref': 'refs/heads/tl/release/candidate',
                'before': '0' * 40, 'after': self.third, 'deleted': False}
        for changes in ({'ref': 'refs/heads/tl/release-other/candidate'},
                        {'ref': 'refs/heads/tl/release/'}, {'deleted': True},
                        {'after': '0' * 40}, {'repository': {'full_name': 'fork/repository'}}):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                full_ci_required(dict(push, **changes), 'push', repository)

    def test_release_branches_into_main_run_full_ci_and_others_refuse(self):
        from ci_events import full_ci_required
        repository = 'example/repository'
        sha = self.third

        def pull(head_ref, head_repo=repository, base_ref='main'):
            return {'number': 11, 'repository': {'full_name': repository}, 'pull_request': {
                'base': {'ref': base_ref, 'sha': sha, 'repo': {'full_name': repository}},
                'head': {'ref': head_ref, 'sha': sha, 'repo': {'full_name': head_repo}}}}

        for ref in ('develop', 'release/2026-11-22', 'release/v1.rc-2'):
            with self.subTest(ref=ref):
                self.assertTrue(full_ci_required(pull(ref), 'pull_request', repository))
        self.assertFalse(full_ci_required(pull('release/2026-11-22', base_ref='develop'), 'pull_request', repository))
        for ref in ('feature', 'release/', 'release/-x', 'release/a/b', 'release/a b', 'releases/x', 'release/' + 'x' * 64):
            with self.subTest(ref=ref):
                with self.assertRaises(ValueError):
                    full_ci_required(pull(ref), 'pull_request', repository)
        with self.assertRaises(ValueError):
            full_ci_required(pull('release/2026-11-22', head_repo='fork/repository'), 'pull_request', repository)


if __name__ == '__main__':
    unittest.main()
