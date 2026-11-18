"""PR artifacts are reusable only with exact source, recipe and checksum evidence."""
import hashlib
import json
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import Mock, patch

from staging_cd_artifacts import ci_workflow, qualifies, try_reuse
from staging_cd_versions import DEPLOYABLES
from staging_cd_common import Refusal
from staging_cd_receipts_tests import fixture_identity


class ArtifactTests(unittest.TestCase):
    def fixture(self):
        expected = fixture_identity()
        run = {"id": 10, "run_attempt": 1, "event": "pull_request", "status": "completed", "conclusion": "success",
               "head_sha": "f" * 40, "path": ".github/workflows/ci-web.yml", "repository": {"full_name": "ChronoAIProject/cma"}}
        image = b"synthetic tested image"
        metadata = {"schema": "cma-ci-native-image-v1", "component": "web", "sourceRevision": expected["source"],
                    "sourceTree": expected["tree"], "platform": "linux/amd64", "versionLabel": expected["version"],
                    "buildFingerprint": expected["build"], "archive": "cma-web-linux-amd64.docker.tar.gz",
                    "archiveSha256": hashlib.sha256(image).hexdigest(), "archiveBytes": len(image),
                    "workflow": {"repository": "ChronoAIProject/cma", "event": "pull_request", "runId": "10", "runAttempt": "1"}}
        api = Mock()
        artifact = {"id": 20, "name": "ci-native-image-web-" + "a" * 40 + "-10-1", "expired": False}
        api.request.side_effect = [{"merged": True, "merge_commit_sha": expected["source"], "head": {"sha": "f" * 40}},
                                   {"workflow_runs": [run]}, {"total_count": 1, "artifacts": [artifact]}]
        def download(_artifact, destination, _token):
            with zipfile.ZipFile(destination, "w") as output:
                output.writestr("metadata.json", json.dumps(metadata))
                output.writestr(metadata["archive"], image)
        return expected, run, metadata, api, download

    def test_current_pr_manifests_and_changed_version_or_recipe_are_ineligible(self):
        expected, run, metadata, _, _ = self.fixture()
        self.assertTrue(qualifies(metadata, expected, run))
        legacy = dict(metadata)
        legacy.pop("buildFingerprint")
        self.assertFalse(qualifies(legacy, expected, run))
        for field, value in (("sourceTree", "d" * 40), ("sourceRevision", "d" * 40), ("platform", "linux/arm64"),
                             ("versionLabel", "git-fixture"), ("buildFingerprint", "d" * 64), ("component", "api")):
            with self.subTest(field=field):
                self.assertFalse(qualifies(dict(metadata, **{field: value}), expected, run))

    def test_artifacts_are_discovered_only_in_the_owning_workflow(self):
        expected, run, _, api, download = self.fixture()
        self.assertEqual(ci_workflow(expected['component']), 'ci-web.yml')
        self.assertEqual({component: ci_workflow(component) for component in DEPLOYABLES}, {
            'am': 'ci-am.yml', 'image-manager': 'ci-am.yml', 'runtime': 'ci-am.yml',
            'runtime-desktop': 'ci-am.yml', 'runtime-omarchy': 'ci-am.yml',
            'eg': 'ci-eg.yml', 'web': 'ci-web.yml'})
        with self.assertRaises(Refusal):
            ci_workflow('unknown')
        run['path'] = '.github/workflows/ci-am.yml'
        with tempfile.TemporaryDirectory() as directory:
            self.assertIsNone(try_reuse(expected, Path(directory), api, 'fixture', download, Mock()))
        self.assertIn('actions/workflows/ci-web.yml/runs?', api.request.call_args.args[0])

    def test_qualified_artifact_is_converted_and_validated_without_building(self):
        expected, _, _, api, download = self.fixture()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runner = Mock()
            with patch("staging_cd_artifacts.inspect") as inspect:
                path = try_reuse(expected, root, api, "fixture", download, runner)
            self.assertEqual(path, root / "image.oci.tar")
            self.assertEqual(runner.call_count, 1)
            self.assertEqual(runner.call_args.args[0][:2], ["skopeo", "copy"])
            inspect.assert_called_once_with(path, expected, runner, allow_qualified=True)

    def test_unqualified_manifest_falls_back_before_using_image_archive(self):
        expected, _, metadata, api, download = self.fixture()
        metadata.pop("buildFingerprint")
        with tempfile.TemporaryDirectory() as directory:
            runner = Mock()
            self.assertIsNone(try_reuse(expected, Path(directory), api, "fixture", download, runner))
            runner.assert_not_called()
            self.assertFalse((Path(directory) / "qualified.docker.tar.gz").exists())

    def test_substituted_artifact_content_is_rejected_before_conversion(self):
        expected, _, metadata, api, download = self.fixture()
        metadata["archiveSha256"] = "0" * 64
        with tempfile.TemporaryDirectory() as directory:
            runner = Mock()
            with self.assertRaises(Refusal):
                try_reuse(expected, Path(directory), api, "fixture", download, runner)
            runner.assert_not_called()


if __name__ == "__main__":
    unittest.main()
