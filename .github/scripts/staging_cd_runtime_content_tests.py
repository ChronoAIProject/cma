"""Runtime catalogs retain exact image identity and fail-closed classification."""
import copy
import json
import unittest
from unittest.mock import patch

import image_secret_gate as gate
import staging_cd_content_policy_tests as fixtures
from image_content_policy import catalog_index
from image_secrets.common import ScanError


class RuntimeContentTests(unittest.TestCase):
    setUp = fixtures.ContentPolicyTests.setUp
    scanner = fixtures.ContentPolicyTests.scanner

    def configure(self, component, entries=None):
        self.output = self.root / component
        self.entry["component"] = component
        self.catalog["entries"] = [self.entry] if entries is None else entries
        self.catalog_path.write_text(json.dumps(self.catalog))

    def scan(self, component):
        return gate.scan_archive(self.archive, self.output, self.scanner,
                                 blocking_rules="credential", component=component)

    def test_each_runtime_requires_its_own_exact_reviewed_content(self):
        for component in ("runtime", "runtime-omarchy"):
            with self.subTest(component=component):
                self.configure(component)
                with patch.object(gate, "load_content_policy", return_value=(self.catalog_path, "a" * 64)):
                    self.assertEqual(self.scan(component), gate.checksum(self.archive))
                report = json.loads((self.output / "REPORT.json").read_text())
                self.assertEqual(report["findings"], self.report["findings"])
                self.assertEqual(report["counts"], {"detected": 1, "blocking": 0, "advisory": 0,
                                                    "reviewed": 1, "allowlisted": 0})

    def test_one_runtime_cannot_borrow_the_other_runtime_catalog(self):
        for component, other in (("runtime", "runtime-omarchy"), ("runtime-omarchy", "runtime")):
            with self.subTest(component=component):
                self.configure(component, [dict(self.entry, component=other)])
                with patch.object(gate, "load_content_policy", return_value=(self.catalog_path, "a" * 64)), \
                        self.assertRaises(gate.ScanRefusal) as error:
                    self.scan(component)
                self.assertEqual(error.exception.evidence["counts"]["blocking"], 1)

    def test_runtime_unknown_real_and_changed_candidates_remain_blocking(self):
        for component in ("runtime", "runtime-omarchy"):
            for failure in ("unknown", "REAL", "file", "candidate", "path"):
                with self.subTest(component=component, failure=failure):
                    row = dict(self.entry, component=component)
                    if failure == "REAL":
                        row["classification"] = "REAL"
                    elif failure in ("file", "candidate"):
                        row[failure + "_sha256"] = "b" * 64
                    elif failure == "path":
                        row["path"] = "etc/other.pem"
                    self.configure(component, [] if failure == "unknown" else [row])
                    self.output = self.root / (component + "-" + failure)
                    with patch.object(gate, "load_content_policy", return_value=(self.catalog_path, "a" * 64)), \
                            self.assertRaises(gate.ScanRefusal) as error:
                        self.scan(component)
                    self.assertEqual(error.exception.evidence["counts"]["blocking"], 1)

    def test_unapproved_runtime_catalog_never_reaches_the_resolver(self):
        for component in ("runtime", "runtime-omarchy"):
            with self.subTest(component=component):
                self.configure(component)
                with patch.object(gate, "load_content_policy", return_value=None), \
                        patch("image_content_policy.resolve") as resolver, \
                        self.assertRaises(gate.ScanRefusal):
                    self.scan(component)
                resolver.assert_not_called()

    def test_incomplete_runtime_scan_never_enters_content_resolution(self):
        for component in ("runtime", "runtime-omarchy"):
            for outcome in ("scanner_error", "scanner_timeout", "scanner_unavailable"):
                with self.subTest(component=component, outcome=outcome):
                    self.configure(component)
                    def refuse(*_args):
                        raise gate.ScanRefusal("Synthetic scanner failure", outcome)
                    with patch.object(gate, "load_content_policy", return_value=(self.catalog_path, "a" * 64)), \
                            patch("image_content_policy.resolve") as resolver, self.assertRaises(gate.ScanRefusal):
                        gate.scan_archive(self.archive, self.output, refuse,
                                          blocking_rules="credential", component=component)
                    resolver.assert_not_called()

    def test_private_key_rules_cannot_receive_public_runtime_classifications(self):
        for component in ("runtime", "runtime-omarchy"):
            catalog = copy.deepcopy(self.catalog)
            catalog["entries"][0].update(component=component, rule="content.private-key-header")
            with self.subTest(component=component), self.assertRaises(ScanError):
                catalog_index(catalog, self.report)


if __name__ == "__main__":
    unittest.main()
