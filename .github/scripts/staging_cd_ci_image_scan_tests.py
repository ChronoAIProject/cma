"""The CI image scan runs the staging gate on the exported image and explains refusals."""
import contextlib
import hashlib
import inspect
import io
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import ci_image_secret_scan as ci_scan
import staging_cd_artifacts
from image_scan_evidence import project
from image_secret_gate import ScanRefusal

REVISION = "a" * 40


def digest(text):
    return hashlib.sha256(text.encode()).hexdigest()


class CIImageScanTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.export = self.root / "ci-native-image"
        self.export.mkdir()
        archive = self.export / "cma-api-linux-amd64.docker.tar.gz"
        archive.write_bytes(b"synthetic exported image")
        self.metadata = {"component": "api", "sourceRevision": REVISION, "archive": archive.name,
                         "archiveSha256": hashlib.sha256(archive.read_bytes()).hexdigest(),
                         "archiveBytes": archive.stat().st_size}
        self.write_metadata()
        self.summary = self.root / "summary.md"
        self.env = patch.dict("os.environ", {"RUNNER_TEMP": str(self.root), "GITHUB_STEP_SUMMARY": str(self.summary)})
        self.env.start()
        self.addCleanup(self.env.stop)

    def write_metadata(self, **changes):
        (self.export / "metadata.json").write_text(json.dumps(dict(self.metadata, **changes)))

    def run_main(self, component="api", scanner=None, convert=None):
        calls = []

        def fake_scan(archive, output, *, component, source):
            calls.append((archive.name, component, source.name))
            if scanner is not None:
                scanner()
            return "b" * 64, ("1.0.4", "c" * 64)

        stdout = io.StringIO()
        with patch.object(ci_scan, "convert", convert or (lambda archive, output: output.write_bytes(b"oci"))), \
                patch.object(ci_scan, "source_snapshot", lambda revision, destination: destination.mkdir()), \
                patch.object(ci_scan, "verified_staging_scan", fake_scan), contextlib.redirect_stdout(stdout):
            status = ci_scan.main([component, str(self.export), REVISION])
        return status, stdout.getvalue(), calls

    def test_components_cover_every_exported_image_and_use_publication_names(self):
        source = Path(ci_scan.__file__).with_name("export-ci-native-image.py").read_text()
        self.assertIn('choices=("api", "web", "runtime", "runtime-omarchy", "cmaeg")', source)
        self.assertEqual(ci_scan.COMPONENTS, {"web": "web", "api": "am", "cmaeg": "eg", "runtime": "runtime",
                                              "runtime-omarchy": "runtime-omarchy"})

    def test_clean_scan_uses_the_publication_component_and_removes_its_workspace(self):
        status, output, calls = self.run_main()
        self.assertEqual(status, 0, output)
        self.assertEqual(calls, [("image.oci.tar", "am", "source")])
        self.assertEqual([p.name for p in self.root.iterdir() if p.name.startswith("ci-image-scan-")], [])

    def test_conversion_matches_the_staging_reuse_conversion(self):
        recorded = []
        with patch.object(ci_scan.subprocess, "run", lambda args, **kwargs: recorded.append((args, kwargs))):
            ci_scan.convert(Path("/in/image.docker.tar.gz"), Path("/out/image.oci.tar"))
        args, kwargs = recorded[0]
        self.assertEqual(args, ["skopeo", "copy", "--format", "oci", "docker-archive:/in/image.docker.tar.gz",
                                "oci-archive:/out/image.oci.tar"])
        self.assertTrue(kwargs["check"])
        self.assertIn('["skopeo", "copy", "--format", "oci", "docker-archive:" + str(target), "oci-archive:" + str(output)]',
                      inspect.getsource(staging_cd_artifacts.try_reuse))

    def test_changed_archive_or_foreign_metadata_never_reaches_the_scanner(self):
        for changes in ({"archiveSha256": "d" * 64}, {"archiveBytes": 1}, {"component": "web"},
                        {"sourceRevision": "e" * 40}):
            with self.subTest(changes=sorted(changes)):
                self.write_metadata(**changes)
                status, output, calls = self.run_main()
                self.assertEqual((status, calls), (1, []))
                self.assertIn("stage=prepare", output)
                self.assertIn("::error title=Image secret scan did not run for am::", output)

    def test_blocking_findings_name_rule_layer_policy_paths_and_the_fix(self):
        binary = {"scope": "layer", "image": 0, "layer": 2, "entry": 7, "rule": "gitleaks.generic-api-key",
                  "path": "path-sha256:" + digest("usr/local/bin/cma"), "candidate_sha256": digest("candidate"),
                  "file_sha256": digest("file")}
        other = dict(binary, path="path-sha256:" + digest("etc/private/thing"), layer=3,
                     rule="gitleaks.image-secret-assignment", candidate_sha256=digest("other"))
        report = {"complete": True, "counts": {"detected": 9, "blocking": 2, "advisory": 6, "reviewed": 1},
                  "blocking_findings": [binary, other]}

        def refuse():
            error = ScanRefusal("Image secret scan has blocking findings", "scanner_findings")
            error.evidence = project(report, "scanner_findings")
            raise error

        status, output, _ = self.run_main(scanner=refuse)
        self.assertEqual(status, 1)
        self.assertIn("FAIL ci-image-scan component=am outcome=scanner_findings blocking=2 detected=9 reviewed=1", output)
        self.assertIn("rule=gitleaks.generic-api-key scope=layer layer=2 path=usr/local/bin/cma "
                      f"candidate_sha256={digest('candidate')}", output)
        self.assertIn(f"path=path-sha256:{digest('etc/private/thing')}", output)
        self.assertNotIn("etc/private/thing", output.replace("path-sha256:" + digest("etc/private/thing"), ""))
        self.assertIn("binary_candidates", output)
        self.assertIn("APPROVED_CONTENT_POLICY", output)
        self.assertIn("::error title=Image secret scan refused am::scanner_findings", output)
        self.assertIn("Image secret scan refused `am`", self.summary.read_text())

    def test_incomplete_scan_reports_failure_without_printing_partial_matches(self):
        raw_report = {"schema": "image-secret-scan-v1", "status": "ERROR", "complete": False,
                      "exit_code": 2, "error": "classification-operation-failed",
                      "findings": [{"rule": "gitleaks.generic-api-key", "path": "private-path",
                                    "candidate_sha256": digest("synthetic-value")}],
                      "omitted_findings": 2}

        def refuse():
            error = ScanRefusal("private error text", "scanner_error")
            error.evidence = project(raw_report, error.outcome)
            raise error

        status, output, _ = self.run_main(scanner=refuse)
        self.assertEqual(status, 1)
        self.assertIn("Scan incomplete; partial detections=3", output)
        self.assertIn("diagnostic=classification_failed", output)
        self.assertIn("diagnostic_sha256=" + digest(raw_report["error"]), output)
        self.assertNotIn("candidate_sha256=", output)
        self.assertNotIn("private error text", output)
        self.assertNotIn("private-path", output)
        self.assertNotIn(raw_report["error"], output)
        self.assertIn("diagnostic=classification_failed", self.summary.read_text())

    def test_every_scanner_outcome_has_owner_facing_advice(self):
        for outcome in ("scanner_timeout", "scanner_error", "scanner_unavailable", "receipt_invalid",
                        "content_refused", "scan_refused", "unlisted"):
            with self.subTest(outcome=outcome):
                def refuse():
                    raise ScanRefusal("fixture", outcome)
                status, output, _ = self.run_main(scanner=refuse)
                self.assertEqual(status, 1)
                expected = outcome if outcome in ci_scan.ADVICE else "scan_refused"
                self.assertIn(f"outcome={expected}", output)
                self.assertIn(ci_scan.ADVICE[expected][0], output)

    def test_conversion_failures_are_named_and_bounded(self):
        def times_out(archive, output):
            raise subprocess.TimeoutExpired(["skopeo", "copy"], ci_scan.CONVERT_SECONDS)

        def fails(archive, output):
            raise subprocess.CalledProcessError(1, ["skopeo", "copy"])

        status, output, calls = self.run_main(convert=times_out)
        self.assertEqual((status, calls), (1, []))
        self.assertIn("stage=convert reason=command_timeout", output)
        status, output, calls = self.run_main(convert=fails)
        self.assertEqual((status, calls), (1, []))
        self.assertIn("stage=convert reason=CalledProcessError", output)


if __name__ == "__main__":
    unittest.main()
