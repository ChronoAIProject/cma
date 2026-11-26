import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from image_secrets.backend import Gitleaks
from image_secrets.common import ScanError, digest_bytes
from image_secrets.detector_report import report_rows
from image_secrets.policy import Findings


class DetectorReportTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="detector-report-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.report = self.root / "gitleaks-private.json"

    def test_oversized_backend_report_imports_every_row_in_canonical_order(self):
        stage = self.root / "stage"
        stage.mkdir()
        physical = stage / "entry-00000000.txt"
        physical.write_text("synthetic fixture")
        location = {"scope": "layer", "image": 0, "layer": 0, "entry": 0, "path": "fixture"}
        count = 66
        padding = "x" * (1024 * 1024)

        def fake_detector(*args, **kwargs):
            with self.report.open("w", encoding="utf-8") as stream:
                stream.write("[")
                for index in reversed(range(count)):
                    if index != count - 1:
                        stream.write(",")
                    json.dump({"File": str(physical), "RuleID": "fixture",
                               "Secret": f"synthetic-{index}", "StartLine": index,
                               "Match": padding}, stream)
                stream.write("]")
            self.assertGreater(self.report.stat().st_size, 64 * 1024 * 1024)
            kwargs["stderr"].write(f"WRN leaks found: {count}\n".encode())
            return subprocess.CompletedProcess([], 1)

        with patch("subprocess.run", return_value=subprocess.CompletedProcess([], 0, b"8.30.1")):
            backend = Gitleaks("gitleaks", self.root / "rules", self.root, 5)
        findings = Findings(None)
        observed = []
        with patch("subprocess.run", side_effect=fake_detector):
            self.assertTrue(backend.detect(stage, {physical: location}, findings, observed=observed))
        self.assertEqual(len(findings.items), count)
        self.assertEqual(len(observed), count)
        expected = {digest_bytes(f"synthetic-{index}".encode()) for index in range(count)}
        self.assertEqual({item["candidate_sha256"] for item in findings.items}, expected)
        self.assertEqual([item["candidate_sha256"] for item in findings.items], sorted(expected))
        self.assertEqual([row["candidate_sha256"] for row in observed], sorted(expected))
        self.assertFalse(self.report.exists())

    def test_backend_withholds_rows_from_invalid_or_inconsistent_reports(self):
        stage = self.root / "stage"
        stage.mkdir()
        physical = stage / "entry-00000000.txt"
        physical.write_text("synthetic fixture")
        location = {"scope": "layer", "image": 0, "layer": 0, "entry": 0, "path": "fixture"}
        valid_row = json.dumps({"File": str(physical), "RuleID": "fixture",
                                "Secret": "synthetic", "StartLine": 1})
        with patch("subprocess.run", return_value=subprocess.CompletedProcess([], 0, b"8.30.1")):
            backend = Gitleaks("gitleaks", self.root / "rules", self.root, 5)
        for payload, code, warning in (
            (f"[{valid_row},", 1, b"WRN leaks found: 1\n"),
            (f"[{valid_row}]{{}}", 1, b"WRN leaks found: 1\n"),
            (f"[{valid_row}]", 0, b""),
            ("[]", 1, b"WRN leaks found: 1\n"),
            (f"[{valid_row}]", 1, b"WRN leaks found: 2\n"),
            (f"[{valid_row}]", 1, b""),
            (f"[{valid_row}]", 1, b"WRN leaks found: 1\nWRN leaks found: 1\n"),
        ):
            def fake_detector(*args, **kwargs):
                self.report.write_text(payload)
                kwargs["stderr"].write(warning)
                return subprocess.CompletedProcess([], code)

            findings = Findings(None)
            observed = []
            with patch("subprocess.run", side_effect=fake_detector), self.assertRaises(ScanError):
                backend.detect(stage, {physical: location}, findings, observed=observed)
            self.assertEqual(findings.items, [])
            self.assertEqual(observed, [])

    def test_boundaries_and_strict_json(self):
        with patch("image_secrets.detector_report.READ_CHARACTERS", 7):
            self.report.write_text(' \t[ {"n":"hello, ☃"},\r\n {"n":2} ] \n', encoding="utf-8")
            self.assertEqual(list(report_rows(self.report)), [{"n": "hello, ☃"}, {"n": 2}])
            self.report.write_text(" [ ] \n")
            self.assertEqual(list(report_rows(self.report)), [])
            for payload in (
                b'{"n":1}', b'[1]', b'[{}', b'[{},]', b'[{} {}]', b'[]{}',
                b'[{"n":1,"n":2}]', b'[{"n":NaN}]', b'[{"n":"\xff"}]',
            ):
                with self.subTest(payload=payload):
                    self.report.write_bytes(payload)
                    with self.assertRaises(ScanError):
                        list(report_rows(self.report))

    def test_budget_refuses_entire_report(self):
        self.report.write_text('[{"n":"fixture"}]')
        with patch("image_secrets.detector_report.MAX_REPORT_BYTES", 8):
            with self.assertRaisesRegex(ScanError, "gitleaks-report-too-large"):
                list(report_rows(self.report))
        with patch("image_secrets.detector_report.MAX_ROW_CHARACTERS", 8):
            with self.assertRaisesRegex(ScanError, "gitleaks-report-row-too-large"):
                list(report_rows(self.report))


if __name__ == "__main__":
    unittest.main()
