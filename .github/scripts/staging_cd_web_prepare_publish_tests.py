"""The standalone publisher only writes a scanned, immutable Web auxiliary image."""

import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import Mock, patch

import staging_cd_web_prepare_publish as publication
from staging_cd_common import Refusal


class WebPreparePublicationTests(unittest.TestCase):
    def test_version_is_resolved_once_and_metadata_uses_the_immutable_image(self):
        digest = "sha256:" + "a" * 64
        registry = Mock()
        registry.manifest.return_value = (digest, {})
        labels = {"org.opencontainers.image.version": "0.22.12",
                  "org.opencontainers.image.revision": "b" * 40}
        runner = Mock(return_value=json.dumps({"config": {"Labels": labels}}).encode())
        auth = Path("public-auth.json")
        target = publication.release("0.22.12", registry, auth, runner)
        registry.manifest.assert_called_once_with(publication.WEB_REPOSITORY, "0.22.12")
        self.assertEqual(target["image"], publication.WEB_REPOSITORY + "@" + digest)
        self.assertEqual(runner.call_args.args[0][-1], "docker://" + target["image"])
        with self.assertRaises(Refusal):
            publication.release("latest", registry, auth, runner)
        with self.assertRaises(Refusal):
            publication.release("0.22.70", registry, auth, runner)

    def test_publication_verifies_scanned_bytes_before_login_and_reads_back_the_digest(self):
        with TemporaryDirectory() as root:
            directory = Path(root)
            archive = directory / "prepare.oci.tar"
            archive.write_bytes(b"synthetic image")
            scanned = publication.checksum(archive)
            digest = "sha256:" + "a" * 64
            result = {"prepare": {"archive": archive, "digest": digest, "archive_sha256": scanned},
                      "incumbent_digest": "sha256:" + "b" * 64, "candidate_digest": "sha256:" + "c" * 64,
                      "asset_manifest_sha256": "d" * 64, "asset_count": 3}
            registry = Mock()
            runner = Mock()
            events = Mock()
            events.attach_mock(runner, "transport")
            with patch.object(publication, "prepare", return_value=result), \
                    patch.object(publication, "verified_staging_scan", return_value=("wrong", None)) as scan, \
                    patch.object(publication, "copy_auxiliary") as copy:
                events.attach_mock(scan, "scan")
                events.attach_mock(copy, "copy")
                with self.assertRaises(Refusal):
                    publication.publish("0.22.12", "0.22.70", directory, "actor", "fixture",
                                        registry=registry, runner=runner)
                runner.assert_not_called()
                copy.assert_not_called()
                events.reset_mock()
                scan.return_value = (scanned, None)
                receipt = publication.publish("0.22.12", "0.22.70", directory, "actor", "fixture",
                                              registry=registry, runner=runner)
                self.assertEqual([call[0] for call in events.mock_calls], ["scan", "transport", "copy"])
                self.assertEqual(copy.call_args.args[0], (archive, digest, scanned))
                self.assertEqual(receipt["image"], publication.WEB_REPOSITORY + "@" + digest)
                self.assertEqual(receipt["tag"], "asset-sha256-" + digest[7:])
                registry.exists.assert_called_once_with(publication.WEB_REPOSITORY, digest, settle=True)
                self.assertFalse((directory / "registry-auth.json").exists())


if __name__ == "__main__":
    unittest.main()
