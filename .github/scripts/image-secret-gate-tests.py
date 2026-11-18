#!/usr/bin/env python3
"""Publication binds complete receipts, advisory entropy and reviewed decisions."""
import copy
import hashlib
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import image_secret_gate as gate


class GateTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.archive = self.root / "image.tar"
        self.archive.write_bytes(b"opaque image archive; scanner transport is injected")
        self.output = self.root / "report"
        bundle = patch.object(gate, "verify_bundle")
        bundle.start()
        self.addCleanup(bundle.stop)

    def receipt(self, **changes):
        report = {"schema": "image-secret-scan-v1", "complete": True, "status": "CLEAN", "exit_code": 0,
                  "findings": [], "allowlisted": [], "archive_sha256": gate.checksum(self.archive),
                  "gitleaks_version": "8.30.1", "scanner_version": "1.0.3", "blocking_rules": "all",
                  "blocking_findings": [], "advisory_findings": [], "reviewed_classifications": [],
                  "counts": {"detected": 0, "blocking": 0, "advisory": 0, "reviewed": 0, "allowlisted": 0},
                  "policy": {"allowlist_sha256": hashlib.sha256(gate.EMPTY_POLICY.encode()).hexdigest(),
                             "gitleaks_rules_sha256": gate.checksum(gate.SCANNER.parent / "gitleaks.toml")}}
        report.update(changes)
        self.output.mkdir()
        (self.output / "REPORT.json").write_text(json.dumps(report))

    def test_exact_archive_and_empty_exception_policy(self):
        def scan(*args):
            self.assertEqual(args[:3], ("bash", str(gate.SCANNER), str(self.archive)))
            self.assertEqual(args[args.index("--input-kind") + 1], "archive")
            self.assertEqual(args[args.index("--blocking-rules") + 1], "all")
            self.assertNotIn("--classifications", args)
            self.assertEqual(args[args.index("--expected-archive-sha256") + 1], gate.checksum(self.archive))
            policy = Path(args[args.index("--allowlist") + 1])
            self.assertEqual(json.loads(policy.read_text()), {"version": 1, "entries": []})
            self.receipt()
        self.assertEqual(gate.scan_archive(self.archive, self.output, scan), gate.checksum(self.archive))
        self.assertEqual(list(self.root.glob("image-scan-policy-*")), [])

    def test_incomplete_findings_and_exceptions_fail_closed(self):
        for change in ({"complete": False}, {"status": "FINDINGS"}, {"exit_code": 1},
                       {"findings": [{"rule": "fixture"}]}, {"allowlisted": [{"rule": "fixture"}]},
                       {"archive_sha256": "wrong"}, {"schema": "other"}, {"gitleaks_version": "8.25.0"},
                       {"policy": {"allowlist_sha256": "wrong"}}, {"scanner_version": "1.0.1"},
                       {"blocking_rules": "credential"}, {"exit_code": False}, {"error": "incomplete"},
                       {"counts": {"detected": False, "blocking": 0, "advisory": 0, "reviewed": 0, "allowlisted": 0}}):
            def scan(*_args, change=change):
                self.receipt(**change)
            with self.subTest(change=change), self.assertRaises(gate.ScanRefusal):
                gate.scan_archive(self.archive, self.output, scan)
            (self.output / "REPORT.json").unlink()
            self.output.rmdir()

    def test_staging_explicitly_records_entropy_as_advisory(self):
        entropy = {"finding_id": "a" * 64, "rule": "gitleaks.image-high-entropy"}
        def scan(*args):
            self.assertEqual(args[args.index("--blocking-rules") + 1], "credential")
            self.receipt(blocking_rules="credential", findings=[entropy], advisory_findings=[entropy],
                         counts={"detected": 1, "blocking": 0, "advisory": 1, "reviewed": 0, "allowlisted": 0})
        self.assertEqual(gate.scan_staging_archive(self.archive, self.output, scan), gate.checksum(self.archive))

    def test_staging_policy_a_does_not_change_default_release_policy(self):
        for scan_archive, expected in ((gate.scan_staging_archive, "credential"), (gate.scan_archive, "all")):
            def scan(*args, expected=expected):
                self.assertEqual(args[args.index("--blocking-rules") + 1], expected)
                self.receipt(blocking_rules=expected)
            with self.subTest(expected=expected):
                scan_archive(self.archive, self.output, scan)
            (self.output / "REPORT.json").unlink()
            self.output.rmdir()

    def test_strict_entropy_and_unclassified_credentials_or_unknown_rules_block(self):
        cases = [("all", "gitleaks.image-high-entropy")]
        cases += [("credential", rule) for rule in (
            "content.private-key-header", "gitleaks.jwt", "gitleaks.github-pat", "gitleaks.aws-access-token",
            "content.credential-url", "gitleaks.generic-api-key", "gitleaks.image-secret-assignment",
            "gitleaks.image-hex-secret-assignment", "path.key-or-certificate", "unknown-rule")]
        for mode, rule in cases:
            finding = {"finding_id": "a" * 64, "rule": rule}
            def scan(*_args, mode=mode, finding=finding):
                self.receipt(blocking_rules=mode, findings=[finding], advisory_findings=[finding],
                             counts={"detected": 1, "blocking": 0, "advisory": 1, "reviewed": 0, "allowlisted": 0})
            with self.subTest(mode=mode, rule=rule), self.assertRaises(gate.ScanRefusal):
                gate.scan_archive(self.archive, self.output, scan, blocking_rules=mode)
            (self.output / "REPORT.json").unlink()
            self.output.rmdir()

    def test_advisory_partition_and_counts_cannot_omit_or_duplicate_findings(self):
        entropy = {"finding_id": "a" * 64, "rule": "gitleaks.image-high-entropy"}
        base = {"blocking_rules": "credential", "findings": [entropy], "advisory_findings": [entropy],
                "counts": {"detected": 1, "blocking": 0, "advisory": 1, "reviewed": 0, "allowlisted": 0}}
        for change in ({"advisory_findings": []}, {"findings": [entropy, entropy]}, {"counts": {}},
                       {"reviewed_classifications": [dict(entropy, classification="public-material")]},
                       {"findings": [{"finding_id": "invalid", "rule": entropy["rule"]}]}):
            data = dict(base, **change)
            with self.subTest(change=change), self.assertRaises(gate.ScanRefusal):
                gate.scan_staging_archive(self.archive, self.output, lambda *_args, data=data: self.receipt(**data))
            (self.output / "REPORT.json").unlink()
            self.output.rmdir()

    def test_reviewed_packet_is_copied_exactly_and_bound_into_receipt(self):
        finding = {"finding_id": "a" * 64, "rule": "path.key-or-certificate"}
        packet = json.dumps({"review": {"report_sha256": "b" * 64},
                             "entries": [{"finding_id": finding["finding_id"], "classification": "public-material"}]}).encode()
        def scan(*args):
            supplied = Path(args[args.index("--classifications") + 1])
            self.assertEqual(supplied.read_bytes(), packet)
            self.receipt(blocking_rules="credential", findings=[finding],
                         reviewed_classifications=[dict(finding, classification="public-material")],
                         counts={"detected": 1, "blocking": 0, "advisory": 0, "reviewed": 1, "allowlisted": 0})
            path = self.output / "REPORT.json"
            report = json.loads(path.read_text())
            report["policy"].update(classifications_sha256=hashlib.sha256(packet).hexdigest(),
                                    classification_review_sha256="b" * 64)
            path.write_text(json.dumps(report))
        with patch.object(gate, "load_classifications", return_value=packet):
            self.assertEqual(gate.scan_staging_archive(self.archive, self.output, scan), gate.checksum(self.archive))
        self.assertEqual(list(self.root.glob("image-scan-policy-*")), [])

    def test_real_entropy_and_forged_review_receipts_block(self):
        finding = {"finding_id": "a" * 64, "rule": "gitleaks.image-high-entropy"}
        self.receipt(blocking_rules="credential", findings=[finding], advisory_findings=[finding],
                     counts={"detected": 1, "blocking": 0, "advisory": 1, "reviewed": 0, "allowlisted": 0})
        base = json.loads((self.output / "REPORT.json").read_text())
        for classification in ("REAL", "unapproved", "public-material"):
            packet = json.dumps({"review": {"report_sha256": "b" * 64},
                                 "entries": [{"finding_id": "a" * 64, "classification": classification}]}).encode()
            report = copy.deepcopy(base)
            report["policy"].update(classifications_sha256=hashlib.sha256(packet).hexdigest(),
                                    classification_review_sha256="b" * 64)
            with self.subTest(classification=classification), self.assertRaises(gate.ScanRefusal):
                gate.validate_receipt(report, gate.checksum(self.archive), "credential", packet)
        for field in ("classifications_sha256", "classification_review_sha256"):
            report = copy.deepcopy(base)
            report["policy"][field] = "b" * 64
            with self.subTest(field=field), self.assertRaises(gate.ScanRefusal):
                gate.validate_receipt(report, gate.checksum(self.archive), "credential", None)

    def test_invalid_mode_unreviewed_packet_and_existing_receipt_never_run_scanner(self):
        runner = Mock()
        with self.assertRaises(gate.ScanRefusal):
            gate.scan_archive(self.archive, self.output, runner, blocking_rules="ignore")
        with patch.object(gate, "load_classifications", side_effect=gate.PolicyRefusal("unreviewed")), \
             self.assertRaises(gate.ScanRefusal):
            gate.scan_staging_archive(self.archive, self.output, runner)
        self.output.mkdir()
        with self.assertRaises(gate.ScanRefusal):
            gate.scan_archive(self.archive, self.output, runner)
        runner.assert_not_called()

    def test_missing_receipt_or_archive_substitution_fails_closed(self):
        with self.assertRaises(gate.ScanRefusal):
            gate.scan_archive(self.archive, self.output, Mock())
        def substitute(*_args):
            self.receipt()
            self.archive.write_bytes(b"different archive")
        with self.assertRaises(gate.ScanRefusal):
            gate.scan_archive(self.archive, self.output, substitute)

    def test_changed_or_missing_bundle_is_never_executed(self):
        runner = Mock()
        with patch.object(gate, "verify_bundle", side_effect=gate.PolicyRefusal("changed")), \
             self.assertRaises(gate.ScanRefusal):
            gate.scan_archive(self.archive, self.output, runner)
        runner.assert_not_called()

    def test_credentials_are_not_given_to_scanner(self):
        with patch.dict(os.environ, {"GH_TOKEN": "fixture", "CD_API_KEY": "fixture"}), \
             patch.object(gate.subprocess, "run", return_value=Mock(returncode=0)) as run:
            gate.run_scan("scanner")
            self.assertNotIn("GH_TOKEN", run.call_args.kwargs["env"])
            self.assertNotIn("CD_API_KEY", run.call_args.kwargs["env"])

    def test_failure_and_timeout_never_expose_tool_output(self):
        with patch.object(gate.subprocess, "run", return_value=Mock(returncode=1, stdout=b"private", stderr=b"private")):
            with self.assertRaises(gate.ScanRefusal) as caught:
                gate.run_scan("scanner")
            self.assertNotIn("private", str(caught.exception))
        with patch.object(gate.subprocess, "run", side_effect=subprocess.TimeoutExpired("scanner", 1)), \
             self.assertRaises(gate.ScanRefusal):
            gate.run_scan("scanner")


if __name__ == "__main__":
    unittest.main()
