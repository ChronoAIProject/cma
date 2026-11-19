"""Publication-only repairs select all active images in PR and cached CD plans."""

from unittest.mock import patch
import unittest

import batch_selection
from deployable_manifest import CURRENT
from image_inputs import changed_input_maps
from selection_cache import SelectionCache
import selection
import test_batch_selection
import test_images


class PublicationInputsTests(unittest.TestCase):
    setUp = test_images.ImageTests.setUp
    write = test_images.ImageTests.write
    pin_recipes = test_images.ImageTests.pin_recipes
    selected = test_images.ImageTests.selected

    def test_scanner_policy_resolver_and_publisher_changes_select_all(self):
        for path in (
            '.github/scripts/image-content-policy.json',
            '.github/scripts/image-content-manifest.json',
            '.github/scripts/image_content_identity.py',
            '.github/scripts/image_content_policy.py',
            '.github/scripts/image_scan_evidence.py',
            '.github/scripts/image_scanner_policy.py',
            '.github/scripts/image_secret_gate.py',
            '.github/scripts/image_bundle_contexts.py',
            '.github/scripts/image_bundle_syntax.cjs',
            '.github/scripts/image-content-vendor/babel-parser.cjs',
            '.github/scripts/image-secret-scanner/image_secrets/engine.py',
            '.github/scripts/image-secret-scanner/SOURCE-MANIFEST.json',
            '.github/scripts/image-secret-scanner/gitleaks.toml',
            '.github/image-secret-classifications/packet.json',
            '.github/scripts/install-image-gitleaks.sh',
            '.github/scripts/staging_cd_publish.py',
            '.github/scripts/staging_cd_publication_diagnostics.py',
            '.github/scripts/staging_cd_build.py',
            '.github/scripts/staging_cd_build_cache.py',
            '.github/scripts/staging_cd_registry.py',
            '.github/scripts/staging_cd_contract.py',
            '.github/scripts/staging_cd_compatibility.py',
            'api/crates/layer3-adapter/runtime-versions.json',
            '.github/scripts/api-cargo-cache.py',
            '.github/workflows/staging-release.yml',
        ):
            with self.subTest(path=path):
                self.assertEqual(self.selected(path), sorted(CURRENT))

    def test_unrelated_ci_tests_docs_and_similar_paths_do_not_publish(self):
        self.assertEqual(self.selected(
            '.github/workflows/ci.yml', '.github/workflows/ci-images.yml',
            '.github/scripts/ci_tracks/gate.py',
            '.github/scripts/image-scanner-policy-tests.py',
            '.github/scripts/staging_cd_publication_cache_tests.py',
            '.github/scripts/image_secret_gate_tests.py',
            '.github/scripts/image-manager-mongo-contract.py',
            '.github/workflows/staging-release.yml.notes',
            'README.md'), [])

    def test_historical_seven_image_summaries_select_only_current_five(self):
        from deployable_manifest import RETIRED
        manifest = dict(self.manifest, **{name: {} for name in RETIRED})
        summary = ({name: set() for name in manifest}, manifest)
        self.assertEqual(changed_input_maps(
            ['.github/scripts/image_secret_gate.py'], [summary, summary]), sorted(CURRENT))


class PublicationHistoryTests(unittest.TestCase):
    setUp = test_batch_selection.BatchTests.setUp
    write = test_batch_selection.BatchTests.write
    pin_recipes = test_batch_selection.BatchTests.pin_recipes
    git = test_batch_selection.BatchTests.git
    commit = test_batch_selection.BatchTests.commit

    def test_add_rename_delete_policy_agrees_for_pr_push_batch_and_warm_cache(self):
        base = self.third
        self.write('.github/scripts/image_content_policy.py', 'policy = 1\n')
        added = self.commit()
        self.git('mv', '.github/scripts/image_content_policy.py', '.github/scripts/helper.py')
        renamed = self.commit()
        self.write('.github/scripts/image_content_policy.py', 'policy = 1\n')
        restored = self.commit()
        (self.root / '.github/scripts/image_content_policy.py').unlink()
        deleted = self.commit()
        revisions = [added, renamed, restored, deleted]
        cache = SelectionCache(self.root / 'cache', 'e' * 64)
        batch = batch_selection.determine_many(self.root, revisions, cache)
        for head, actual in zip(revisions, batch):
            event = {'pull_request': {'base': {'sha': base}, 'head': {'sha': head}}}
            pull = selection.determine(self.root, event, 'pull_request')
            push = selection.determine(self.root, {'after': head}, 'push')
            self.assertEqual(pull['deployables'], sorted(CURRENT))
            self.assertEqual(push, actual)
            self.assertEqual(actual['deployables'], sorted(CURRENT))
            base = head
        with patch.object(batch_selection, 'summarize', side_effect=AssertionError('must reuse')):
            self.assertEqual(batch_selection.determine_many(self.root, revisions, cache), batch)


if __name__ == '__main__':
    unittest.main()
