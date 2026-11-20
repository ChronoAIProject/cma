"""Refuse transformed and under-specified Gitleaks parser-marker decisions."""

import copy
import json
import sys
import tarfile
import unittest
from pathlib import Path
from unittest.mock import patch

from image_secrets import key_marker_view
from image_secrets.common import digest_bytes
from image_secrets.gate import FORMAT_MARKER_ADVISORY, KEY_MARKER_VIEW_ADVISORY, KEY_MARKER_VIEW_RULE
from tests import fixtures
from tests import test_package_policy as packages

SCRIPTS = Path(__file__).resolve().parents[2]
FILE = key_marker_view.PATHS["shared-mime-info"]
BEGIN = b"-----BEGIN " + b"PRIVATE KEY-----"
END = b"-----END " + b"PRIVATE KEY-----"
RECORDS = b'"/>\n  <record type="source-format"/>\n  <record end="'
PREFIX = b'<record begin="'
SUFFIX = b'"/>\n'


class KeyMarkerViewTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    detector_scan = packages.PackageTests.detector_scan
    manifest = packages.PackageTests.manifest

    def fixture(self, middle=RECORDS, end_marker=END, *, suffix=SUFFIX):
        candidate = BEGIN + middle + end_marker
        contents = PREFIX + candidate + suffix
        layer = fixtures.tar_bytes([(FILE, contents), ("etc/machine-id", b""),
                                    ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id"))])
        archive = self.root / "marker-view.tar"
        fixtures.docker_archive(archive, [layer])
        raw = self.detector_scan(archive, key_marker_view.SCOPE)
        [image] = raw["inventory"]["images"]
        [finding] = [row for row in raw["blocking_findings"] if row["rule"] == KEY_MARKER_VIEW_RULE
                     and row["candidate_sha256"] == digest_bytes(candidate)]
        manifest = self.manifest(fixtures.tar_bytes([(FILE, contents)]), layer, file=FILE, component=key_marker_view.SCOPE)
        manifest["packages"][0].update(name="shared-mime-info", authority_type="arch-signature", architecture="x86_64")
        [row] = manifest["entries"]
        span = {"offset": len(PREFIX), "size": len(candidate), "sha256": digest_bytes(candidate)}
        proof = {
            "finding_id": finding["finding_id"], "original": span, "detector": copy.deepcopy(span),
            "parts": [{"kind": kind, "size": len(body), "sha256": digest_bytes(body),
                       "source": "data/format-records.xml", "line": i, "proof_sha256": digest_bytes(body)}
                      for i, (kind, body) in enumerate((("begin", BEGIN), ("records", middle), ("end", end_marker)), 1)],
        }
        row["key_marker_view"] = {"archive_sha256": raw["archive_sha256"], "image_config": image["config_digest"],
                                  "image_manifest": image["manifest"], "matches": [proof]}
        row["matches"] = [{"rule": KEY_MARKER_VIEW_RULE, "candidate_sha256": span["sha256"],
                           "line": finding.get("line"), "explanation": KEY_MARKER_VIEW_ADVISORY, "proof_sha256": "4" * 64}]
        return archive, manifest, raw, proof

    def refused(self, archive, manifest, name, *, options=(), classification="scanner_findings"):
        with patch.object(sys, "path", [str(SCRIPTS), *sys.path]):
            import image_secret_gate as gate

            packet = json.dumps(dict(manifest, scanner_manifest_sha256=gate.MANIFEST_SHA256)).encode()
            with (patch.object(gate, "load_package_evidence", return_value=(packet, digest_bytes(packet), packages.REVIEW)),
                  patch.object(gate, "scanner_options", return_value=list(options)),
                  self.assertRaises(gate.ScanRefusal) as result):
                gate.verified_staging_scan(archive, self.root / name, component=key_marker_view.SCOPE)
            self.assertEqual(result.exception.outcome, classification)
            return getattr(result.exception, "package_report", None)

    def test_exact_advisory_preserves_sibling_blockers_and_cache_parity(self):
        archive, manifest, raw, proof = self.fixture()
        cache = self.root / "key-cache.json"
        options = ["--detector-cache-context", "b" * 64, "--detector-cache-output", str(cache)]
        cold = self.refused(archive, manifest, "cold", options=options)
        warm = self.refused(archive, manifest, "warm", options=[*options, "--detector-cache-input", str(cache)])
        self.assertEqual(warm["detector_cache"]["hits"], 1)
        self.assertEqual(warm["package_evidence"], cold["package_evidence"])
        self.assertEqual(cold["findings"], raw["findings"])
        [advisory] = cold["package_evidence"]["entries"]
        self.assertEqual(advisory["finding_id"], proof["finding_id"])
        self.assertEqual(advisory["classification"], KEY_MARKER_VIEW_ADVISORY)
        self.assertEqual([item["rule"] for item in cold["blocking_findings"]], ["content.private-key-header"])

    def test_a_rewritten_view_or_different_intervening_bytes_refuse(self):
        archive, manifest, raw, _ = self.fixture()
        proposal = copy.deepcopy(manifest)
        proposal["entries"][0]["key_marker_view"]["matches"][0]["detector"]["offset"] += 1
        self.assertIsNone(self.refused(archive, proposal, "rendered", classification="content_refused"))
        proposal = copy.deepcopy(manifest)
        proposal["entries"][0]["key_marker_view"]["matches"][0]["parts"][1]["sha256"] = "f" * 64
        report = self.refused(archive, proposal, "changed")
        self.assertEqual(report["blocking_findings"], raw["blocking_findings"])
        self.assertEqual(report["package_evidence"]["entries"], [])

    def test_publication_verifies_both_independent_marker_decisions(self):
        archive, manifest, raw, _ = self.fixture()
        row = manifest["entries"][0]
        [marker] = [f for f in raw["blocking_findings"] if f["rule"] == "content.private-key-header"]
        context = PREFIX + BEGIN + RECORDS + END + SUFFIX
        row["reviewed_literals"] = {key: row["key_marker_view"][key]
                                    for key in ("archive_sha256", "image_config", "image_manifest")}
        row["reviewed_literals"]["findings"] = [{
            "rule": marker["rule"], "finding_id": marker["finding_id"],
            "spans": [{"offset": len(PREFIX), "size": len(BEGIN), "sha256": digest_bytes(BEGIN),
                       "context": {"offset": 0, "size": len(context), "sha256": digest_bytes(context)},
                       "source": {"path": "data/format-records.xml", "line": 1, "sha256": digest_bytes(context)}}],
        }]
        row["matches"].append({"rule": marker["rule"], "candidate_sha256": None, "line": None,
                               "explanation": FORMAT_MARKER_ADVISORY, "proof_sha256": digest_bytes(context)})
        with patch.object(sys, "path", [str(SCRIPTS), *sys.path]):
            import image_secret_gate as gate

            packet = json.dumps(dict(manifest, scanner_manifest_sha256=gate.MANIFEST_SHA256)).encode()
            with patch.object(gate, "load_package_evidence", return_value=(packet, digest_bytes(packet), packages.REVIEW)):
                output = self.root / "verified"
                gate.verified_staging_scan(archive, output, component=key_marker_view.SCOPE)
        verified = json.loads((output / "REPORT.json").read_bytes())
        self.assertEqual(verified["findings"], raw["findings"])
        self.assertEqual(verified["blocking_findings"], [])
        self.assertEqual({r["classification"] for r in verified["package_evidence"]["entries"]},
                         {FORMAT_MARKER_ADVISORY, KEY_MARKER_VIEW_ADVISORY})

    def test_private_envelopes_and_duplicate_candidates_cannot_supply_proof(self):
        _, _, _, proof = self.fixture()
        candidate = BEGIN + RECORDS + END
        self.assertFalse(key_marker_view.verified_span(PREFIX + candidate + candidate, b"", proof))
        self.assertFalse(key_marker_view.verified_span(PREFIX + candidate, candidate, proof))
        between = b"\n" + b"ABCD" * 10 + b"\n"
        opaque = BEGIN + between + END
        proof["original"].update(size=len(opaque), sha256=digest_bytes(opaque))
        proof["parts"][1].update(size=len(between), sha256=digest_bytes(between))
        self.assertFalse(key_marker_view.verified_span(PREFIX + opaque, b"", proof))

    def test_a_second_begin_boundary_can_be_an_independent_public_marker(self):
        archive, manifest, raw, proof = self.fixture(end_marker=BEGIN)
        report = self.refused(archive, manifest, "second-begin")
        self.assertEqual(report["findings"], raw["findings"])
        self.assertEqual([row["finding_id"] for row in report["package_evidence"]["entries"]], [proof["finding_id"]])

    def test_receipt_requires_the_complete_span_to_fit_in_the_member(self):
        archive, manifest, raw, proof = self.fixture(suffix=b"")
        exact = self.refused(archive, manifest, "exact-eof")
        self.assertEqual([row["finding_id"] for row in exact["package_evidence"]["entries"]], [proof["finding_id"]])

        proof["original"]["size"] += 1
        proof["detector"]["size"] += 1
        proof["parts"][-1]["size"] += 1
        overrun = self.refused(archive, manifest, "past-eof")
        self.assertEqual(overrun["blocking_findings"], raw["blocking_findings"])
        self.assertEqual(overrun["package_evidence"]["entries"], [])
