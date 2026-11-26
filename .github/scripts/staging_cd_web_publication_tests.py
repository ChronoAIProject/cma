"""Verify the Web publisher's custody and interrupted publication without live writes."""

from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import Mock, patch

from image_scanner_policy import MANIFEST_SHA256
from image_secret_gate import checksum
from staging_cd_common import Refusal
from staging_cd_receipts_tests import fixture_identity
from staging_cd_web_overlay import REPOSITORY, encoded
from staging_cd_web_overlay_tests import fixture_archive
from staging_cd_web_publication import publish, resume


class WebPublicationTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.first, _, _ = fixture_archive(self.root, "a" * 40, "0.22.8")
        self.previous, _, _ = fixture_archive(self.root, "b" * 40, "0.22.9")
        self.candidate, _, _ = fixture_archive(self.root, "c" * 40, "0.22.10")
        self.identity = fixture_identity(source=self.candidate.source, previous=self.previous.version,
                                         version=self.candidate.version)
        base = REPOSITORY + "@" + self.previous.digest
        self.incumbent = {"image": base, "base_image": base, "source_sha": self.previous.source,
                          "version": self.previous.version}
        self.scope = {"incumbent": self.incumbent, "preimage_sha256": "f" * 64, "inputs": [
            {"image": REPOSITORY + "@" + self.first.digest, "source_sha": self.first.source,
             "version": self.first.version, "completed_rollouts": 1},
        ]}
        self.registry = Mock()
        self.registry.manifest.side_effect = lambda _, reference, **_options: self.tags.get(reference)
        self.registry.exists.side_effect = lambda _, digest, **_options: digest in {row[0] for row in self.tags.values()}
        self.tags = {item.digest: (item.digest, {}) for item in (self.first, self.previous)}
        self.events = []
        self.fail_version = False
        self.fail_pull = False
        self.stages = []

    def runner(self, args, **_options):
        if args[1] == "login":
            self.events.append("login")
            Path(args[3]).touch()
        else:
            self.assertEqual(args[1:4], ["copy", "--preserve-digests", "--src-authfile"])
            self.assertEqual(Path(args[4]).stat().st_mode & 0o777, 0o600)
            if self.fail_pull:
                raise Refusal("registry pull denied")
            expected = args[5].split("@")[1]
            item = {item.digest: item for item in (self.first, self.previous)}[expected]
            shutil.copyfile(item.archive, Path(args[6].removeprefix("oci-archive:")))

    def scan(self, archive, report_path, *, component, source):
        self.assertEqual((component, source), ("web", self.root))
        self.events.append("scan:" + archive.name)
        digest = checksum(archive)
        report_path.mkdir()
        (report_path / "REPORT.json").write_bytes(encoded({
            "archive_sha256": digest, "counts": {key: 0 for key in
                                                 ("detected", "blocking", "advisory", "reviewed", "allowlisted")},
        }))
        return digest, ("1.0.4", MANIFEST_SHA256)

    def push(self, archive, repository, reference, digest, auth, registry, runner, reservation_check, retry):
        self.assertTrue(archive.is_file())
        self.assertEqual(repository, REPOSITORY)
        self.assertEqual(auth.stat().st_mode & 0o777, 0o600)
        self.assertTrue(reservation_check())
        self.events.append("push:" + reference)
        self.assertNotIn(reference, self.tags)
        self.tags[reference] = self.tags[digest] = (digest, {})
        if reference == self.identity["version"] and self.fail_version:
            raise Refusal("registry assigned version but the receipt was interrupted")

    def protected_publish(self):
        work = Path(tempfile.mkdtemp(dir=self.root))
        with (patch("staging_cd_web_publication.inspect", return_value=self.candidate.digest),
              patch("staging_cd_web_publication.verified_staging_scan", side_effect=self.scan),
              patch("staging_cd_web_overlay.verified_staging_scan", side_effect=self.scan),
              patch("staging_cd_web_publication.push", side_effect=self.push)):
            return publish(self.candidate.archive, self.identity, self.scope, work, self.registry,
                           "synthetic-publisher", "test-actor", source=self.root, reservation_check=lambda: True,
                           prepared_checksum=checksum(self.candidate.archive), max_files=16, max_bytes=512,
                           runner=self.runner, mark=self.stages.append)

    def test_all_scans_precede_any_write_and_only_promote_owns_the_version(self):
        receipt = self.protected_publish()
        continuity = receipt["asset_continuity"]
        self.assertEqual(self.events[:4], ["scan:c.oci.tar", "login", "scan:web-prepare.oci.tar", "scan:web-promote.oci.tar"])
        self.assertEqual(self.events[4:], ["push:asset-sha256-" + self.candidate.digest[7:],
                                          "push:asset-sha256-" + continuity["prepare_image"].split(":")[-1],
                                          "push:" + self.identity["version"]])
        self.assertEqual(self.tags[self.identity["version"]][0], receipt["digest"])
        self.assertNotEqual(receipt["digest"], self.candidate.digest)
        self.assertFalse(list(self.root.rglob("web-registry-auth.json")))

    def test_denied_pull_cleans_auth_before_any_registry_write(self):
        self.fail_pull = True
        with self.assertRaisesRegex(Refusal, "registry pull denied"):
            self.protected_publish()
        self.assertEqual(self.stages[-2:], ["registry_login", "registry_preflight"])
        self.assertEqual(self.events, ["scan:c.oci.tar", "login"])
        self.assertFalse(list(self.root.rglob("web-registry-auth.json")))

    def test_interrupted_unfinalized_publication_burns_version_instead_of_rewriting_it(self):
        self.fail_version = True
        with self.assertRaisesRegex(Refusal, "receipt was interrupted"):
            self.protected_publish()
        prior_events = self.events.copy()
        with self.assertRaisesRegex(Refusal, "Reserved registry version already exists"):
            self.protected_publish()
        self.assertEqual(self.events, prior_events)

    def test_finalized_receipt_resumes_only_while_one_of_its_phases_serves(self):
        receipt = self.protected_publish()
        prior_events = self.events.copy()
        protected = receipt["asset_continuity"]
        prepared = dict(self.incumbent, image=protected["prepare_image"])
        promoted = {"image": REPOSITORY + "@" + receipt["digest"],
                    "base_image": protected["candidate_base_image"], "source_sha": receipt["source"],
                    "version": receipt["identity"]["version"]}
        for phase in (self.incumbent, prepared, promoted):
            self.assertEqual(resume(receipt, dict(self.scope, incumbent=phase), self.registry), receipt)
        unrelated = dict(promoted, image=REPOSITORY + "@" + self.first.digest)
        with self.assertRaisesRegex(Refusal, "stale controller preimage"):
            resume(receipt, dict(self.scope, incumbent=unrelated), self.registry)
        self.assertEqual(self.events, prior_events)
