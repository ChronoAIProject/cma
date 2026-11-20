"""Published test vectors require reviewed provenance and exact artifact bytes."""

import copy
import json
import unittest
from unittest.mock import patch

import image_secret_gate as gate
import staging_cd_content_policy_tests as fixtures
from image_content_policy import catalog_index
from image_secrets.common import ScanError
from image_secrets.gate import assess


class PublicVectorTests(unittest.TestCase):
    scanner = fixtures.ContentPolicyTests.scanner

    def setUp(self):
        fixtures.ContentPolicyTests.setUp(self)
        self.item = fixtures.finding("etc/example.pem", self.payload, rule="gitleaks.private-key")
        self.report["findings"] = [self.item]
        assess(self.report, "credential")
        self.entry.update(component="runtime", rule="gitleaks.private-key", provenance={
            "package": "example-lib", "package_version": "1.2.3-1", "upstream_tag": "1.2.3",
            "upstream_commit": "a" * 40, "source_file": "lib/crypto-selftests-pk.c",
            "source_line": 12, "source_sha256": "b" * 64,
        })
        self.save_catalog()

    def save_catalog(self):
        self.catalog_path.write_text(json.dumps(self.catalog))

    def scan(self, component="runtime"):
        with patch.object(gate, "load_content_policy", return_value=(self.catalog_path, "a" * 64)):
            return gate.scan_archive(self.archive, self.output, self.scanner,
                                     blocking_rules="credential", component=component)

    def test_reviewed_exact_vector_cannot_override_the_private_key_block(self):
        with self.assertRaises(gate.ScanRefusal):
            self.scan()
        report = json.loads((self.output / "REPORT.json").read_text())
        self.assertEqual(report["findings"], [self.item])
        self.assertEqual(report["counts"]["reviewed"], 0)
        self.assertEqual(report["counts"]["blocking"], 1)
        self.assertEqual(report["allowlisted"], [])

    def test_package_update_with_same_candidate_stops_matching(self):
        updated = self.payload + b" changed library build"
        config = fixtures.content_image(self.archive, updated)
        self.item = fixtures.finding("etc/example.pem", updated, rule="gitleaks.private-key",
                                     candidate_sha256=self.entry["candidate_sha256"])
        self.report.update(findings=[self.item], archive_sha256=gate.checksum(self.archive),
                           inventory={"images": [{"config_digest": "sha256:" + config}]})
        assess(self.report, "credential")
        with self.assertRaises(gate.ScanRefusal):
            self.scan()
        report = json.loads((self.output / "REPORT.json").read_text())
        self.assertEqual(report["counts"]["blocking"], 1)
        self.assertEqual(report["counts"]["reviewed"], 0)

    def test_changed_candidate_path_rule_and_real_classification_refuse(self):
        for field, value in (("candidate_sha256", "c" * 64), ("path", "etc/other.pem"),
                             ("rule", "gitleaks.generic-api-key"), ("classification", "REAL")):
            with self.subTest(field=field):
                original = self.entry[field]
                self.entry[field] = value
                self.save_catalog()
                self.output = self.root / field
                with self.assertRaises(gate.ScanRefusal):
                    self.scan()
                self.assertEqual(json.loads((self.output / "REPORT.json").read_text())["counts"]["blocking"], 1)
                self.entry[field] = original

    def test_other_components_cannot_borrow_runtime_vector_entry(self):
        for component in ("web", "am", "eg", "runtime-omarchy"):
            with self.subTest(component=component):
                self.output = self.root / component
                with self.assertRaises(gate.ScanRefusal):
                    self.scan(component)

    def test_missing_or_malformed_provenance_cannot_approve_key_rules(self):
        for field in self.entry["provenance"]:
            catalog = copy.deepcopy(self.catalog)
            del catalog["entries"][0]["provenance"][field]
            with self.subTest(field=field), self.assertRaises(ScanError):
                catalog_index(catalog, self.report)
        for field, value in (("source_line", True), ("source_line", 0), ("source_sha256", "invalid"),
                             ("source_file", "../other.c"), ("upstream_commit", "branch-name"),
                             ("package", "*")):
            catalog = copy.deepcopy(self.catalog)
            catalog["entries"][0]["provenance"][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(ScanError):
                catalog_index(catalog, self.report)

    def test_private_vector_scope_and_other_private_rules_stay_rejected(self):
        for field, value in (("component", "web"), ("component", "runtime-omarchy"),
                             ("scope", "config"), ("rule", "content.private-key-header")):
            catalog = copy.deepcopy(self.catalog)
            catalog["entries"][0][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(ScanError):
                catalog_index(catalog, self.report)

    def test_unapproved_vectors_cannot_reach_resolution(self):
        with patch.object(gate, "load_content_policy", return_value=None), \
                patch("image_content_policy.resolve") as resolver, self.assertRaises(gate.ScanRefusal):
            gate.scan_archive(self.archive, self.output, self.scanner,
                              blocking_rules="credential", component="runtime")
        resolver.assert_not_called()


if __name__ == "__main__":
    unittest.main()
