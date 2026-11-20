"""Check publication decisions for a reviewed distribution configuration span."""

import copy
import json
import sys
import unittest
from base64 import urlsafe_b64encode
from pathlib import Path
from unittest.mock import patch

from image_secrets.common import digest_bytes
from image_secrets.gate import DISTRIBUTION_ADVISORY, DISTRIBUTION_RULE
from tests import test_executable_vectors as executable
from tests import test_package_policy as packages

COMPONENT = "runtime-omarchy"
SCRIPTS = Path(__file__).resolve().parents[2]


def synthetic_value(seed):
    return b"AI" + b"za" + urlsafe_b64encode(bytes.fromhex(digest_bytes(seed)))[:35]


class DistributionConfigurationTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    manifest = packages.PackageTests.manifest
    detector_scan = packages.PackageTests.detector_scan
    executable_fixture = executable.ExecutableVectorTests.fixture

    def fixture(self, *, extra=b"", installed=True):
        value = synthetic_value(b"catalogued distribution fixture")
        suffix = b"\x00" + value + b"\x00" + extra
        archive, manifest, report = self.executable_fixture(suffix=suffix, installed=installed, component=COMPONENT)
        digest = digest_bytes(value)
        [finding] = [row for row in report["blocking_findings"]
                     if row["rule"] == DISTRIBUTION_RULE and row["candidate_sha256"] == digest]
        row = manifest["entries"][0]
        test_vector = row["self_test"]
        [vector] = test_vector["vectors"]
        source_digest = digest_bytes(b"fixture build recipe")
        provenance_digest = digest_bytes(b"reviewed independent distribution fixture")
        row["matches"].append({"rule": DISTRIBUTION_RULE, "candidate_sha256": digest, "line": None,
                               "explanation": DISTRIBUTION_ADVISORY, "proof_sha256": provenance_digest})
        binding = {key: test_vector["executable"][key] for key in ("archive_sha256", "image_config", "image_manifest")}
        row["distribution_config"] = {**binding, "finding_id": finding["finding_id"],
            "database": copy.deepcopy(test_vector["database"]), "provenance_sha256": provenance_digest,
            "source": {"path": "PKGBUILD", "definition_line": 1, "build_argument_line": 2,
                       "published_sha256": source_digest, "build_recipe_sha256": source_digest},
            "span": {"offset": vector["offset"] + vector["size"] + 1, "size": len(value), "sha256": digest},
        }
        return archive, manifest, report

    def publish(self, archive, manifest, name, *, expected_error=None, options=(), wrong_pin=False):
        with patch.object(sys, "path", [str(SCRIPTS), *sys.path]):
            import image_secret_gate as gate

            pinned = copy.deepcopy(manifest)
            pinned["scanner_manifest_sha256"] = gate.MANIFEST_SHA256
            raw = json.dumps(pinned).encode()
            evidence = raw, "f" * 64 if wrong_pin else digest_bytes(raw), packages.REVIEW
            output = self.root / name
            with (patch.object(gate, "load_package_evidence", return_value=evidence),
                  patch.object(gate, "scanner_options", return_value=list(options))):
                if expected_error is None:
                    gate.verified_staging_scan(archive, output, component=COMPONENT)
                    return json.loads((output / "REPORT.json").read_bytes())
                with self.assertRaises(gate.ScanRefusal) as refusal:
                    gate.verified_staging_scan(archive, output, component=COMPONENT)
                self.assertEqual(refusal.exception.outcome, expected_error)
                return getattr(refusal.exception, "package_report", None)

    def test_publication_reconstructs_independent_proofs_on_cold_and_warm_scans(self):
        archive, manifest, raw = self.fixture()
        cache = self.root / "native-cache.json"
        options = ["--detector-cache-context", "d" * 64, "--detector-cache-output", str(cache)]
        cold = self.publish(archive, manifest, "cold", options=options)
        warm = self.publish(archive, manifest, "warm", options=[*options, "--detector-cache-input", str(cache)])
        self.assertEqual(warm["detector_cache"]["hits"], 1)
        self.assertEqual(warm["package_evidence"], cold["package_evidence"])
        self.assertEqual(warm["findings"], cold["findings"])
        self.assertEqual(cold["findings"], raw["findings"])
        rows = {row["rule"]: row for row in cold["package_evidence"]["entries"]}
        self.assertEqual(set(rows), {executable.RULE, DISTRIBUTION_RULE})
        self.assertEqual(rows[DISTRIBUTION_RULE]["classification"], DISTRIBUTION_ADVISORY)
        self.assertEqual(rows[DISTRIBUTION_RULE]["distribution_config"],
                         {"public_path": executable.EXECUTABLE, **manifest["entries"][0]["distribution_config"]})
        for key, rule in (("distribution_config", DISTRIBUTION_RULE), ("self_test", executable.RULE)):
            with self.subTest(missing=key):
                partial = copy.deepcopy(manifest)
                row = partial["entries"][0]
                del row[key]
                row["matches"] = [match for match in row["matches"] if match["rule"] != rule]
                refused = self.publish(archive, partial, key, expected_error="scanner_findings")
                self.assertEqual(refused["findings"], raw["findings"])
                self.assertEqual([entry["rule"] for entry in refused["blocking_findings"]], [rule])
                self.assertEqual({entry["rule"] for entry in refused["package_evidence"]["entries"]}, rows.keys() - {rule})

    def test_distribution_proof_refuses_missing_or_changed_binding(self):
        archive, manifest, raw = self.fixture()
        for key, value in (("archive_sha256", "f" * 64), ("image_config", "sha256:" + "f" * 64),
                           ("image_manifest", "sha256:" + "f" * 64), ("finding_id", "f" * 64),
                           ("span", None), ("candidate", "f" * 64)):
            with self.subTest(key=key):
                proposal = copy.deepcopy(manifest)
                row = proposal["entries"][0]
                proof = row["distribution_config"]
                if key == "span":
                    proof["span"]["offset"] += 1
                elif key == "candidate":
                    proof["span"]["sha256"] = row["matches"][-1]["candidate_sha256"] = value
                else:
                    proof[key] = value
                refused = self.publish(archive, proposal, key, expected_error="scanner_findings")
                self.assertEqual(refused["findings"], raw["findings"])
                self.assertEqual([row["rule"] for row in refused["blocking_findings"]], [DISTRIBUTION_RULE])
                self.assertEqual([row["rule"] for row in refused["package_evidence"]["entries"]], [executable.RULE])

    def test_publication_refuses_a_wrong_build_recipe_or_review_pin(self):
        archive, manifest, _ = self.fixture()
        for key in ("recipe", "provenance", "catalog"):
            with self.subTest(key=key):
                proposal = copy.deepcopy(manifest)
                proof = proposal["entries"][0]["distribution_config"]
                if key == "recipe":
                    proof["source"]["build_recipe_sha256"] = "f" * 64
                elif key == "provenance":
                    proof["provenance_sha256"] = "f" * 64
                self.assertIsNone(self.publish(archive, proposal, key, expected_error="content_refused",
                                               wrong_pin=key == "catalog"))

    def test_repeated_uncovered_or_missing_membership_stay_blocking(self):
        for name, extra, installed, expected in (
            ("duplicate", synthetic_value(b"catalogued distribution fixture"), True, {executable.RULE}),
            ("unreviewed", synthetic_value(b"unreviewed distribution fixture"), True,
             {executable.RULE, DISTRIBUTION_RULE}),
            ("missing-database", b"", False, set()),
        ):
            with self.subTest(case=name):
                archive, manifest, raw = self.fixture(extra=extra, installed=installed)
                refused = self.publish(archive, manifest, name, expected_error="scanner_findings")
                self.assertEqual(refused["findings"], raw["findings"])
                self.assertEqual({row["rule"] for row in refused["package_evidence"]["entries"]}, expected)
                self.assertIn(DISTRIBUTION_RULE, {row["rule"] for row in refused["blocking_findings"]})
