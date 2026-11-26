"""The independent publisher verifies image identity and scans before writing."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock

from image_secret_gate import ScanRefusal, checksum
from staging_cd_common import Refusal
from staging_cd_journey_probe import COMPONENT, publish
from staging_cd_receipts import IDENTITY_LABEL, canonical
from staging_cd_registry import JOURNEY_REPOSITORY

SOURCE = "a" * 40
RELEASE = dict(schema="cma-journey-image-v1", component=COMPONENT, source=SOURCE, version=SOURCE, tree="b" * 40)
MANIFEST = b'{"schemaVersion":2,"config":{},"layers":[]}'
DIGEST = "sha256:" + hashlib.sha256(MANIFEST).hexdigest()
TAG = DIGEST.replace(":", "-", 1)


class JourneyPublicationTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.directory = Path(directory.name)
        self.archive = self.directory / "image.oci.tar"
        self.archive.write_bytes(b"synthetic archive")
        self.config = {"os": "linux", "architecture": "amd64", "config": {"User": "10001:10001", "Labels": {
            IDENTITY_LABEL: canonical(RELEASE),
            "org.opencontainers.image.source": "https://github.com/ChronoAIProject/cma",
            "org.opencontainers.image.revision": SOURCE,
            "org.opencontainers.image.version": SOURCE,
        }}}
        self.registry = Mock()
        self.registry.manifest.return_value = (DIGEST, {})
        self.registry.exists.return_value = True
        self.runner = Mock(side_effect=self.transport)
        self.scanner = Mock(side_effect=self.scan)

    def transport(self, args, **kwargs):
        if args[1] == "login":
            Path(args[args.index("--authfile") + 1]).touch()
        if args[1] == "inspect":
            return MANIFEST if args[2] == "--raw" else json.dumps(self.config).encode()
        return b""

    def scan(self, archive, output, *, component):
        self.assertEqual(archive, self.archive)
        self.assertEqual(component, COMPONENT)
        output.mkdir()
        (output / "REPORT.json").write_text('{"counts":{"blocking":0}}')
        return checksum(archive), ("1.0.4", "c" * 64)

    def publish(self):
        return publish(self.directory, RELEASE, self.registry, "fixture", "actor",
                       runner=self.runner, scanner=self.scanner)

    def test_scanned_image_is_copied_and_read_back_by_tag_and_digest(self):
        receipt = self.publish()
        self.assertEqual(receipt, json.loads((self.directory / "publication.json").read_text()))
        self.assertEqual(receipt["source"], SOURCE)
        self.assertEqual(receipt["archive_sha256"], checksum(self.archive))
        self.assertEqual(receipt["image"], JOURNEY_REPOSITORY + "@" + DIGEST)
        copy = self.runner.call_args.args[0]
        self.assertIn("--preserve-digests", copy)
        self.assertEqual(copy[-2:], ["oci-archive:" + str(self.archive), "docker://" + JOURNEY_REPOSITORY + ":" + TAG])
        self.registry.manifest.assert_called_once_with(JOURNEY_REPOSITORY, TAG, settle=True)
        self.registry.exists.assert_called_once_with(JOURNEY_REPOSITORY, DIGEST, settle=True)

    def test_refused_scan_or_unbound_archive_never_authenticates_or_publishes(self):
        refusal = ScanRefusal("synthetic refusal")
        refusal.evidence = {"outcome": "scanner_findings"}
        self.scanner.side_effect = refusal
        with self.assertRaises(ScanRefusal):
            self.publish()
        self.assertEqual(json.loads((self.directory / "scan-failure.json").read_text()), refusal.evidence)
        self.assertEqual(self.runner.call_count, 2)
        self.registry.manifest.assert_not_called()

        def change_archive(archive, output, *, component):
            result = self.scan(archive, output, component=component)
            archive.write_bytes(b"different bytes")
            return result

        self.runner.reset_mock()
        self.scanner.side_effect = change_archive
        with self.assertRaisesRegex(Refusal, "differs from the scanned bytes"):
            self.publish()
        self.assertEqual(self.runner.call_count, 2)
        self.registry.manifest.assert_not_called()

    def test_wrong_revision_refuses_before_scan(self):
        self.config["config"]["Labels"]["org.opencontainers.image.revision"] = "d" * 40
        with self.assertRaisesRegex(Refusal, "reserved release identity"):
            self.publish()
        self.scanner.assert_not_called()

    def test_unconfirmed_registry_digest_has_no_publication_receipt(self):
        self.registry.exists.return_value = False
        with self.assertRaisesRegex(Refusal, "did not retain"):
            self.publish()
        self.assertFalse((self.directory / "publication.json").exists())
