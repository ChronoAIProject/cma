"""Check normalized native evidence through cold and warm publication receipts."""

import copy
import subprocess
import tarfile
import unittest
from unittest.mock import patch

from image_secrets import key_marker_view, native_json_key_view
from image_secrets.common import digest_bytes
from image_secrets.gate import KEY_MARKER_VIEW_ADVISORY, KEY_MARKER_VIEW_RULE
from tests import fixtures
from tests import test_key_marker_view as key_tests
from tests import test_package_policy as packages

FILE = key_marker_view.PATHS["file"]
PREFIX = b'published fixture \xc2\xa9\xff\n<begin value="'
MIDDLE = b'"/>\n  <public record="\xff format label"/>\n  <end value="'
SUFFIX = b'"/>\n'


class NativeJSONKeyViewTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    detector_scan = packages.PackageTests.detector_scan
    manifest = packages.PackageTests.manifest
    refused = key_tests.KeyMarkerViewTests.refused

    def fixture(self, *, suffix=SUFFIX, middle=MIDDLE):
        chunks = (key_tests.BEGIN, middle, key_tests.END)
        original = b"".join(chunks)
        normalized = native_json_key_view.render(original)
        contents = PREFIX + original + suffix
        layer = fixtures.tar_bytes([(FILE, contents), ("etc/machine-id", b""),
                                    ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id"))])
        archive = self.root / "native-records.tar"
        fixtures.docker_archive(archive, [layer])
        report = self.detector_scan(archive, key_marker_view.SCOPE)
        [image] = report["inventory"]["images"]
        candidates = (row for row in report["blocking_findings"] if row["rule"] == KEY_MARKER_VIEW_RULE
                      and row["candidate_sha256"] == digest_bytes(normalized))
        finding = min(candidates, key=lambda row: row["line"])
        evidence = self.manifest(fixtures.tar_bytes([(FILE, contents)]), layer, file=FILE, component=key_marker_view.SCOPE)
        evidence["packages"][0].update(name="file", authority_type="arch-signature", architecture="x86_64")
        [entry] = evidence["entries"]
        proof = {"finding_id": finding["finding_id"],
                 "original": {"offset": len(PREFIX), "size": len(original), "sha256": digest_bytes(original)},
                 "detector": {"offset": len(native_json_key_view.render(PREFIX)), "size": len(normalized),
                              "sha256": digest_bytes(normalized)},
                 "parts": [{"kind": kind, "size": len(chunk), "sha256": digest_bytes(chunk),
                            "source": "fixtures/magic", "line": line, "proof_sha256": digest_bytes(chunk)}
                           for line, (kind, chunk) in enumerate(zip(("begin", "records", "end"), chunks), 1)]}
        entry["native_json_key_view"] = {"encoding": native_json_key_view.ENCODING,
                                         "archive_sha256": report["archive_sha256"], "image_config": image["config_digest"],
                                         "image_manifest": image["manifest"], "matches": [proof]}
        entry["matches"] = [{"rule": finding["rule"], "line": finding["line"],
                             "candidate_sha256": finding["candidate_sha256"],
                             "explanation": KEY_MARKER_VIEW_ADVISORY, "proof_sha256": "4" * 64}]
        return archive, evidence, report, proof

    def test_exact_view_reconstructs_identical_cold_and_warm_receipts(self):
        archive, evidence, raw, proof = self.fixture()
        cache = self.root / "native-cache.json"
        options = ["--detector-cache-context", "a" * 64, "--detector-cache-output", str(cache)]
        cold = self.refused(archive, evidence, "cold-native", options=options)
        warm = self.refused(archive, evidence, "warm-native", options=[*options, "--detector-cache-input", str(cache)])
        self.assertEqual(warm["detector_cache"]["hits"], 1)
        self.assertEqual(cold["package_evidence"], warm["package_evidence"])
        self.assertEqual(cold["findings"], raw["findings"])
        [receipt] = cold["package_evidence"]["entries"]
        self.assertEqual(receipt["native_json_key_view"]["original"], proof["original"])
        self.assertEqual(receipt["native_json_key_view"]["detector"], proof["detector"])
        self.assertEqual([item["rule"] for item in cold["blocking_findings"]], ["content.private-key-header"])

    def test_changed_or_uncovered_ranges_keep_each_raw_finding(self):
        archive, evidence, raw, _ = self.fixture()
        for name, change in (("original", "offset"), ("detector", "offset"), ("detector", "size")):
            with self.subTest(view=name, field=change):
                changed = copy.deepcopy(evidence)
                changed["entries"][0]["native_json_key_view"]["matches"][0][name][change] += 1
                report = self.refused(archive, changed, f"changed-{name}-{change}")
                self.assertEqual(report["blocking_findings"], raw["blocking_findings"])
        claim = evidence["entries"][0]["native_json_key_view"]["matches"][0]
        claim["parts"][-1]["size"] += 1
        claim["parts"][1]["size"] -= 1
        report = self.refused(archive, evidence, "uncovered-record-byte")
        self.assertEqual(report["package_evidence"]["entries"], [])
        self.assertEqual(report["blocking_findings"], raw["blocking_findings"])

    def test_literal_replacement_character_does_not_supply_a_unique_view(self):
        second = native_json_key_view.render(key_tests.BEGIN + MIDDLE + key_tests.END)
        archive, evidence, raw, _ = self.fixture(suffix=SUFFIX + second + SUFFIX)
        report = self.refused(archive, evidence, "ambiguous-native-view")
        self.assertEqual(report["package_evidence"]["entries"], [])
        self.assertEqual(report["blocking_findings"], raw["blocking_findings"])

    def test_independently_detected_binary_key_prevents_native_advisory(self):
        key = subprocess.run(["openssl", "genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:prime256v1",
                              "-outform", "DER"], capture_output=True, check=True, timeout=10).stdout
        archive, evidence, raw, _ = self.fixture(suffix=SUFFIX + key)
        report = self.refused(archive, evidence, "independent-binary-key")
        self.assertEqual(report["package_evidence"]["entries"], [])
        self.assertEqual(report["blocking_findings"], raw["blocking_findings"])
        self.assertIn("content.binary-private-key", {row["rule"] for row in report["blocking_findings"]})

    def test_incomplete_render_has_no_receipt(self):
        archive, evidence, raw, _ = self.fixture()
        with patch.object(native_json_key_view, "MAX_RENDER_SECONDS", 0):
            report = self.refused(archive, evidence, "expired-render-budget")
        self.assertEqual(report["package_evidence"]["entries"], [])
        self.assertEqual(report["blocking_findings"], raw["blocking_findings"])

    def test_rendered_candidate_limit_applies_after_invalid_byte_expansion(self):
        base = native_json_key_view.render(key_tests.BEGIN + MIDDLE + key_tests.END)
        for size in (32768, 32769):
            with self.subTest(size=size):
                middle = MIDDLE + b"." * (size - len(base))
                archive, evidence, raw, proof = self.fixture(middle=middle)
                original = PREFIX + key_tests.BEGIN + middle + key_tests.END + SUFFIX
                self.assertEqual(proof["detector"]["size"], size)
                valid = size == native_json_key_view.MAX_MATCH_BYTES
                self.assertEqual(native_json_key_view.verified_view(
                    original, native_json_key_view.render(original), b"", b"", proof), valid)
                if valid:
                    report = self.refused(archive, evidence, "exact-rendered-budget")
                    self.assertEqual(report["findings"], raw["findings"])
                    self.assertEqual(len(report["package_evidence"]["entries"]), 1)
                else:
                    self.assertIsNone(self.refused(archive, evidence, "over-rendered-budget",
                                                   classification="content_refused"))
