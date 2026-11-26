"""Scan failure receipts name every rule the pinned scanner can report."""
import re
import unittest
from pathlib import Path

from image_scan_evidence import RULES, project

BUNDLE = Path(__file__).resolve().parent / "image-secret-scanner"


def scanner_rules():
    """Rule names the bundle can emit: its gitleaks ids and its own rule literals."""
    toml = (BUNDLE / "gitleaks.toml").read_text(encoding="utf-8")
    rules = {"gitleaks." + rule for rule in re.findall(r'(?m)^id = "([a-z0-9-]+)"$', toml)}
    for source in sorted((BUNDLE / "image_secrets").glob("*.py")):
        rules.update(re.findall(r'"((?:path|content|config)\.[a-z0-9]+(?:-[a-z0-9]+)+)"',
                                source.read_text(encoding="utf-8")))
    return rules


class ScanEvidenceRuleTests(unittest.TestCase):
    def test_every_scanner_rule_is_a_recognized_receipt_rule(self):
        rules = scanner_rules()
        self.assertIn("gitleaks.image-hex-secret-assignment", rules)
        self.assertGreater(len(rules), 15)
        self.assertEqual(sorted(rules - RULES), [])

    def test_incomplete_diagnostic_is_bounded_and_survives_reprojection(self):
        report = {"schema": "image-secret-scan-v1", "status": "ERROR", "complete": False,
                  "exit_code": 2, "error": "scanner-operation-failed"}
        evidence = project(report, "scanner_error")
        self.assertEqual(evidence["diagnostic"]["kind"], "scan_operation_failed")
        self.assertRegex(evidence["diagnostic"]["sha256"], r"^[0-9a-f]{64}$")
        self.assertEqual(project(evidence, "scanner_error")["diagnostic"], evidence["diagnostic"])
        self.assertNotIn(report["error"], str(evidence))
        report["error"] = "private-value\n::warning::injected"
        self.assertEqual(project(report, "scanner_error")["diagnostic"], {"kind": "diagnostic_unavailable"})
        self.assertEqual(project({}, "scanner_error")["diagnostic"], {"kind": "diagnostic_unavailable"})

    def test_recognized_rules_are_projected_and_others_stay_withheld(self):
        report = {"complete": True, "findings": [
            {"rule": "gitleaks.image-hex-secret-assignment"}, {"rule": "private-marker-rule"}]}
        rows = [row["rule"] for row in project(report, "scan_refused")["findings"]]
        self.assertEqual(rows, ["gitleaks.image-hex-secret-assignment", "unrecognized_rule"])


if __name__ == "__main__":
    unittest.main()
