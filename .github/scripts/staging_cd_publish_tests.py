"""Publication cannot pass a blocking scan or mutate the scanned image bytes."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from image_secret_gate import ScanRefusal, checksum
from staging_cd_common import Refusal
from staging_cd_finalize import finalize
from staging_cd_publish import publish
from staging_cd_receipts_tests import fixture_identity, fixture_receipt


class PublicationTests(unittest.TestCase):
    def fixture(self, directory):
        root = Path(directory)
        archive = root / "image.oci.tar"
        archive.write_bytes(b"fixture image archive")
        digest = "sha256:" + "d" * 64
        registry = Mock()
        registry.manifest.side_effect = [None, (digest, {})]
        registry.exists.return_value = True
        def scanner(path, report):
            report.mkdir()
            (report / "REPORT.json").write_text(json.dumps({"counts": {
                "detected": 2, "blocking": 0, "advisory": 2, "reviewed": 0, "allowlisted": 0}}))
            return checksum(path)
        def runner(args, **kwargs):
            if args[:2] == ["skopeo", "login"]:
                Path(args[3]).write_text("fixture")
            return b""
        return archive, digest, registry, scanner, Mock(side_effect=runner)

    def test_scanned_archive_is_copied_once_with_digest_preservation(self):
        with tempfile.TemporaryDirectory() as directory:
            archive, digest, registry, scanner, runner = self.fixture(directory)
            with patch("staging_cd_publish.inspect", return_value=digest):
                receipt = publish(archive, fixture_identity(), Path(directory), registry, "fixture", "actor",
                                  scanner=scanner, runner=runner)
            self.assertEqual(receipt["digest"], digest)
            self.assertEqual(receipt["archive"], checksum(archive))
            self.assertEqual(receipt["scan"]["counts"]["advisory"], 2)
            copy = [call for call in runner.call_args_list if call.args[0][:2] == ["skopeo", "copy"]]
            self.assertEqual(len(copy), 1)
            self.assertIn("--preserve-digests", copy[0].args[0])
            self.assertIn("oci-archive:" + str(archive), copy[0].args[0])
            self.assertEqual(copy[0].args[0][-1], "docker://ghcr.io/chronoaiproject/cma-web:0.10.1")
            self.assertFalse((Path(directory) / "registry-auth.json").exists())
            self.assertNotIn("fixture", " ".join(runner.call_args_list[0].args[0]))

    def test_blocking_scan_and_changed_archive_never_login_or_push(self):
        with tempfile.TemporaryDirectory() as directory:
            archive, digest, registry, scanner, runner = self.fixture(directory)
            with patch("staging_cd_publish.inspect", return_value=digest), self.assertRaises(ScanRefusal):
                publish(archive, fixture_identity(), Path(directory), registry, "fixture", "actor",
                        scanner=Mock(side_effect=ScanRefusal("fixture")), runner=runner)
            runner.assert_not_called()
        with tempfile.TemporaryDirectory() as directory:
            archive, digest, registry, scanner, runner = self.fixture(directory)
            def changed(path, report):
                original = scanner(path, report)
                path.write_bytes(b"modified fixture")
                return original
            with patch("staging_cd_publish.inspect", return_value=digest), self.assertRaises(Refusal):
                publish(archive, fixture_identity(), Path(directory), registry, "fixture", "actor",
                        scanner=changed, runner=runner)
            runner.assert_not_called()

    def test_existing_registry_tag_is_never_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            archive, digest, registry, scanner, runner = self.fixture(directory)
            registry.manifest.side_effect = [(digest, {})]
            with self.assertRaises(Refusal):
                publish(archive, fixture_identity(), Path(directory), registry, "fixture", "actor", scanner=scanner, runner=runner)
            runner.assert_not_called()

    def test_partial_publication_finalizes_only_successful_receipts(self):
        receipt = fixture_receipt()
        tasks = [{"component": "web", "mode": "resume", "receipt": receipt},
                 {"component": "web", "mode": "publish"}]
        publisher, registry = Mock(), Mock()
        self.assertEqual(finalize(tasks, [receipt], publisher, registry), receipt)
        publisher.finalize.assert_called_once_with(receipt, registry.exists)
        with self.assertRaises(Refusal):
            finalize(tasks, [dict(receipt, digest="sha256:" + "e" * 64)], publisher, registry)


if __name__ == "__main__":
    unittest.main()
