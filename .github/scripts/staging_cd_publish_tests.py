"""Publication cannot pass a blocking scan or mutate the scanned image bytes."""
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import staging_cd_publish as publication
from image_scanner_policy import MANIFEST_SHA256
from image_secret_gate import ScanRefusal, checksum
from staging_cd_common import Refusal
from staging_cd_finalize import finalize
from staging_cd_publish import publish
from staging_cd_receipts_tests import fixture_identity, fixture_receipt, fixture_runtime_receipt


class PublicationTests(unittest.TestCase):
    def test_verified_package_scan_retains_only_the_redacted_receipt(self):
        archive = "a" * 64
        section = {"scope": "staging", "entries": [{"finding_id": "c" * 64}]}
        raw_findings = [{"finding_id": "c" * 64}, {"finding_id": "d" * 64}]
        report = {
            "archive_sha256": archive, "package_evidence": section,
            "policy": {"package_evidence_sha256": "b" * 64},
            "findings": raw_findings, "reviewed_classifications": raw_findings[:1],
            "blocking_findings": raw_findings[1:], "advisory_findings": [],
            "counts": {"detected": 2, "reviewed": 1, "blocking": 1},
            "private_diagnostics": "must remain local",
        }
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            publication.retain_package_evidence(report, root / "package-policy", "2" * 40, "sha256:" + "e" * 64)
            stored = json.loads((root / "package-policy" / (archive + ".json")).read_bytes())
            self.assertEqual(stored["package_evidence"], section)
            self.assertEqual(stored["findings"], raw_findings)
            self.assertEqual(stored["blocking_findings"], raw_findings[1:])
            self.assertNotIn("private_diagnostics", stored)
            with self.assertRaises(Refusal):
                publication.retain_package_evidence(dict(report, archive_sha256="../other"), root, "2" * 40, "")

    def fixture(self, directory):
        root = Path(directory)
        archive = root / "image.oci.tar"
        archive.write_bytes(b"fixture image archive")
        digest = "sha256:" + "d" * 64
        registry = Mock()
        registry.manifest.side_effect = [None, (digest, {})]
        registry.exists.return_value = True
        def scanner(path, report, *, component):
            report.mkdir()
            policy = {"staging_runtime_component": component} if component in ("runtime", "runtime-omarchy") else {}
            (report / "REPORT.json").write_text(json.dumps({"policy": policy, "counts": {
                "detected": 2, "blocking": 0, "advisory": 2, "reviewed": 0, "allowlisted": 0}}))
            return checksum(path), ("1.0.4", MANIFEST_SHA256)
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
                                  reservation_check=lambda: True, scanner=scanner, runner=runner)
            self.assertEqual(receipt["digest"], digest)
            self.assertEqual(receipt["archive"], checksum(archive))
            self.assertEqual(receipt["scan"]["bundle"], MANIFEST_SHA256)
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
                        reservation_check=lambda: True, scanner=Mock(side_effect=ScanRefusal("fixture")), runner=runner)
            runner.assert_not_called()
        with tempfile.TemporaryDirectory() as directory:
            archive, digest, registry, scanner, runner = self.fixture(directory)
            def changed(path, report, *, component):
                original = scanner(path, report, component=component)
                path.write_bytes(b"modified fixture")
                return original
            with patch("staging_cd_publish.inspect", return_value=digest), self.assertRaises(Refusal):
                publish(archive, fixture_identity(), Path(directory), registry, "fixture", "actor",
                        reservation_check=lambda: True, scanner=changed, runner=runner)
            runner.assert_not_called()

    def test_entry_rejects_mixed_component_tasks_before_registry_access(self):
        reservation = fixture_identity(component="runtime-omarchy")
        reservation.pop("build")
        reservation["schema"] = "cma-version-reservation-v1"
        valid = {"component": "web", "mode": "resume", "receipt": fixture_receipt()}
        for invalid in ({"component": "web", "mode": "publish", "reservation": reservation},
                        {"component": "web", "mode": "resume", "receipt": fixture_receipt(component="eg")},
                        dict(valid, component="eg")):
            env = {"CD_COMPONENT": "web", "CD_TASKS": json.dumps([valid, invalid]), "GITHUB_EVENT_NAME": "push",
                   "GITHUB_RUN_ID": "100", "GITHUB_RUN_ATTEMPT": "1"}
            with self.subTest(mode=invalid["mode"]), patch.dict(os.environ, env, clear=True), \
                    patch.object(publication, "event_identity"), patch.object(publication, "Registry") as registry, \
                    patch.object(publication, "try_reuse", return_value=Path("qualified.tar")) as reuse, \
                    patch.object(publication, "publish") as publisher, \
                    patch.object(publication, "run_publication", side_effect=lambda action: action(None, Mock())), \
                    self.assertRaisesRegex(Refusal, "component differs"):
                publication.main()
            registry.assert_not_called()
            reuse.assert_not_called()
            publisher.assert_not_called()

    def test_matching_publish_and_resume_tasks_keep_their_validated_components(self):
        for component in ("web", "am", "eg", "runtime", "runtime-omarchy"):
            receipt = (fixture_runtime_receipt(component) if component.startswith("runtime")
                       else fixture_receipt(component=component))
            reservation = {k: v for k, v in receipt["identity"].items() if k != "build"}
            reservation["schema"] = "cma-version-reservation-v1"
            tasks = [{"component": component, "mode": "publish", "reservation": reservation},
                     {"component": component, "mode": "resume", "receipt": receipt}]
            env = {"CD_COMPONENT": component, "GITHUB_RUN_ID": "100", "GITHUB_RUN_ATTEMPT": "1"}
            with self.subTest(component=component):
                self.assertEqual(publication.validated_tasks(tasks, env), [("publish", reservation), ("resume", receipt)])

    def test_wrong_report_scope_refuses_before_smoke_or_registry_write(self):
        with tempfile.TemporaryDirectory() as directory:
            archive, digest, registry, scan, runner = self.fixture(directory)
            smoke = Mock()
            def wrong_scope(path, output, *, component):
                self.assertEqual(component, "web")
                return scan(path, output, component="runtime")
            with patch("staging_cd_publish.inspect", return_value=digest), self.assertRaisesRegex(Refusal, "component differs"):
                publish(archive, fixture_identity(), Path(directory), registry, "fixture", "actor",
                        reservation_check=lambda: True, scanner=wrong_scope, runner=runner, smoke=smoke)
            smoke.assert_not_called()
            runner.assert_not_called()

    def test_crossed_scanner_evidence_refuses_before_registry_writes(self):
        with tempfile.TemporaryDirectory() as directory:
            archive, digest, registry, scanner, runner = self.fixture(directory)
            def crossed(path, report, *, component):
                scanned, (_, bundle) = scanner(path, report, component=component)
                return scanned, ("1.0.3", bundle)
            with patch("staging_cd_publish.inspect", return_value=digest), self.assertRaises(Refusal):
                publish(archive, fixture_identity(), Path(directory), registry, "fixture", "actor",
                        reservation_check=lambda: True, scanner=crossed, runner=runner)
            runner.assert_not_called()

    def test_existing_registry_tag_is_never_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            archive, digest, registry, scanner, runner = self.fixture(directory)
            registry.manifest.side_effect = [(digest, {})]
            with self.assertRaises(Refusal):
                publish(archive, fixture_identity(), Path(directory), registry, "fixture", "actor", reservation_check=lambda: True, scanner=scanner, runner=runner)
            runner.assert_not_called()

    def test_partial_publication_finalizes_only_successful_receipts(self):
        receipt = fixture_receipt()
        tasks = [{"component": "web", "mode": "resume", "receipt": receipt},
                 {"component": "web", "mode": "publish"}]
        publisher, registry = Mock(), Mock()
        self.assertEqual(finalize(tasks, [receipt], publisher, registry), receipt)
        publisher.finalize.assert_called_once_with(receipt, registry.exists, recovery_oid=None)
        with self.assertRaises(Refusal):
            finalize(tasks, [dict(receipt, digest="sha256:" + "e" * 64)], publisher, registry)


if __name__ == "__main__":
    unittest.main()
