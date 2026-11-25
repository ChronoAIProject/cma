"""Publisher-only repairs select no image; contract evidence selects its group."""

from unittest.mock import patch
import unittest

import batch_selection
from image_inputs import changed_input_maps
from publication_inputs import CONTRACT_FILES, CONTRACT_IMAGES
from selection_cache import SelectionCache
import selection
import test_batch_selection
import test_images

PUBLISHER_PATHS = (
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
    '.github/scripts/staging_cd_cache_compaction.py',
    '.github/scripts/staging_cd_registry.py',
    '.github/scripts/staging_cd_contract.py',
    '.github/scripts/staging_cd_compatibility.py',
    '.github/scripts/staging_cd_reservations.py',
    '.github/scripts/api-cargo-cache.py',
    '.github/scripts/ci_retry.py',
    '.github/workflows/staging-release.yml',
    '.github/workflows/staging-cd.yml',
)


class PublicationInputsTests(unittest.TestCase):
    setUp = test_images.ImageTests.setUp
    write = test_images.ImageTests.write
    pin_recipes = test_images.ImageTests.pin_recipes
    selected = test_images.ImageTests.selected

    def test_scanner_policy_resolver_and_publisher_changes_select_no_image(self):
        # Rebuilding a lane whose latest final has every input change only
        # recreates identical bytes. Allocation retries lanes left behind.
        for path in PUBLISHER_PATHS:
            with self.subTest(path=path):
                self.assertEqual(self.selected(path), [])
        self.assertEqual(self.selected(*PUBLISHER_PATHS), [])

    def test_contract_evidence_selects_exactly_the_contract_group(self):
        # The fixture's runtime-versions.json is outside every build context,
        # and the runtime recipe is outside the Agent Manager's. Both still
        # select the complete group, which a contract transition requires.
        self.assertEqual(self.selected('api/crates/layer3-adapter/runtime-versions.json'), sorted(CONTRACT_IMAGES))
        self.assertEqual(self.selected('api/crates/layer4-runtime/Dockerfile'), sorted(CONTRACT_IMAGES))
        self.assertEqual(self.selected('api/crates/layer3-adapter/runtime-versions.json', 'web/src/main.tsx'),
                         sorted(CONTRACT_IMAGES | {'cma-web'}))
        self.assertEqual(len(CONTRACT_FILES), 2)

    def test_unrelated_ci_tests_docs_and_similar_paths_do_not_publish(self):
        self.assertEqual(self.selected(
            '.github/workflows/ci.yml', '.github/workflows/ci-images.yml',
            '.github/scripts/ci_tracks/gate.py',
            '.github/scripts/image-scanner-policy-tests.py',
            '.github/scripts/staging_cd_publication_cache_tests.py',
            '.github/scripts/image_secret_gate_tests.py',
            '.github/scripts/image-manager-mongo-contract.py',
            '.github/workflows/staging-release.yml.notes',
            'api/crates/layer3-adapter/runtime-versions.json.notes',
            'README.md'), [])

    def test_historical_seven_image_summaries_select_only_current_contract_group(self):
        from deployable_manifest import RETIRED
        manifest = dict(self.manifest, **{name: {} for name in RETIRED})
        summary = ({name: set() for name in manifest}, manifest)
        self.assertEqual(changed_input_maps(['.github/scripts/image_secret_gate.py'], [summary, summary]), [])
        self.assertEqual(changed_input_maps(sorted(CONTRACT_FILES), [summary, summary]), sorted(CONTRACT_IMAGES))


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
        self.write('api/crates/layer3-adapter/runtime-versions.json', '{}\n')
        contract = self.commit()
        revisions = [added, renamed, restored, deleted, contract]
        expected = [[], [], [], [], sorted(CONTRACT_IMAGES)]
        cache = SelectionCache(self.root / 'cache', 'e' * 64)
        batch = batch_selection.determine_many(self.root, revisions, cache)
        for head, actual, deployables in zip(revisions, batch, expected):
            event = {'pull_request': {'base': {'sha': base}, 'head': {'sha': head}}}
            pull = selection.determine(self.root, event, 'pull_request')
            push = selection.determine(self.root, {'after': head}, 'push')
            self.assertEqual(pull['deployables'], deployables)
            self.assertEqual(push, actual)
            self.assertEqual(actual['deployables'], deployables)
            base = head
        with patch.object(batch_selection, 'summarize', side_effect=AssertionError('must reuse')):
            self.assertEqual(batch_selection.determine_many(self.root, revisions, cache), batch)


if __name__ == '__main__':
    unittest.main()
