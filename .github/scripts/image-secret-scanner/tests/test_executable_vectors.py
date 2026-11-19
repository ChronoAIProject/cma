"""Check exact image and package bindings for a reviewed executable test vector."""

import copy
import hashlib
import json
import sys
import tarfile
import unittest
from pathlib import Path
from unittest.mock import patch

from image_secrets.common import ScanError, digest_bytes
from image_secrets.gate import EXECUTABLE_TEST_ADVISORY

from tests import fixtures
from tests import test_library_vectors as libraries
from tests import test_package_policy as packages

EXECUTABLE = "usr/lib/fixture/fixture"
ELF = libraries.ELF[:16] + b"\x02\x00" + libraries.ELF[18:]
DATABASE = "var/lib/dpkg/info/fixture.md5sums"
RULE = "content.binary-private-key"


class ExecutableVectorTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    manifest = packages.PackageTests.manifest
    policy = staticmethod(packages.PackageTests.policy)
    classify = packages.PackageTests.classify
    detector_scan = packages.PackageTests.detector_scan

    def fixture(self, *, mode=0o755, extra_key=False, installed=True, include_text=False):
        vector = libraries.LibraryVectorTests.generated_key("-outform", "DER")
        extra = libraries.LibraryVectorTests.generated_key("-outform", "DER") if extra_key else b""
        public_text = b"\n" + packages.PUBLIC_SOURCE if include_text else b""
        executable = ELF + vector + extra + public_text
        md5 = hashlib.md5(executable, usedforsecurity=False).hexdigest()
        database = f"{md5}  {EXECUTABLE}\n".encode() if installed else b""
        entries = [(EXECUTABLE, executable), (DATABASE, database), ("etc/machine-id", b""),
                   ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id"))]
        layer = fixtures.tar_bytes(entries, modes={EXECUTABLE: mode})
        archive = self.root / "executable.tar"
        fixtures.docker_archive(archive, [layer])
        report = self.detector_scan(archive)
        [finding] = [row for row in report["blocking_findings"] if row["rule"] == RULE]
        [image] = report["inventory"]["images"]
        manifest = self.manifest(fixtures.tar_bytes([(EXECUTABLE, executable)]), layer, file=EXECUTABLE)
        manifest["packages"][0].update(name="fixture", authority_type="debian-index", architecture="amd64")
        row = manifest["entries"][0]
        row["matches"] = [{"rule": RULE, "candidate_sha256": None, "line": None,
                           "explanation": EXECUTABLE_TEST_ADVISORY, "proof_sha256": "3" * 64}]
        if include_text:
            [text] = [item for item in report["blocking_findings"] if item["rule"] == packages.RULE]
            self.assertEqual(text["candidate_sha256"], digest_bytes(packages.SYMBOL))
            row["matches"].append({"rule": text["rule"], "candidate_sha256": text["candidate_sha256"],
                                   "line": text.get("line"), "explanation": "caller-supplied-value-expression",
                                   "proof_sha256": digest_bytes(packages.PUBLIC_SOURCE)})
        row["self_test"] = {
            "database": {"path": DATABASE, "layer": 0, "diff_id": row["diff_id"], "entry": 1,
                         "sha256": digest_bytes(database), "member_checksum": md5, "proof_sha256": "4" * 64},
            "upstream": {"name": "fixture", "version": "1.0", "archive_sha256": "5" * 64,
                         "authority_sha256": "6" * 64},
            "vectors": [{"offset": len(ELF), "size": len(vector), "sha256": digest_bytes(vector), "kind": "binary",
                         "source_member": "virtual-test.cc", "source_line": 1, "self_test_symbol": "FixtureVector"}],
            "executable": {"archive_sha256": report["archive_sha256"], "image_config": image["config_digest"],
                           "image_manifest": image["manifest"], "finding_ids": [finding["finding_id"]]},
        }
        return archive, manifest, report

    def test_publication_reconstructs_the_executable_vector_receipt(self):
        archive, manifest, raw = self.fixture()
        scripts = Path(__file__).resolve().parents[2]
        with patch.object(sys, "path", [str(scripts), *sys.path]):
            import image_secret_gate as gate

            manifest["scanner_manifest_sha256"] = gate.MANIFEST_SHA256
            evidence = json.dumps(manifest).encode()
            output = self.root / "executable-scan"
            with patch.object(gate, "load_package_evidence", return_value=(
                    evidence, digest_bytes(evidence), packages.REVIEW)):
                gate.verified_staging_scan(archive, output, component="runtime")
            report = json.loads((output / "REPORT.json").read_bytes())
            self.assertEqual(report["findings"], raw["findings"])
            self.assertEqual(report["counts"]["blocking"], 0)
            [tested] = report["package_evidence"]["entries"]
            self.assertEqual(tested["classification"], EXECUTABLE_TEST_ADVISORY)
            self.assertEqual(tested["executable_self_test"]["executable_path"], EXECUTABLE)

    def test_publication_requires_both_proofs_for_two_findings_in_one_executable(self):
        archive, manifest, raw = self.fixture(include_text=True)
        findings = {row["rule"]: row["finding_id"] for row in raw["blocking_findings"]}
        scripts = Path(__file__).resolve().parents[2]
        with patch.object(sys, "path", [str(scripts), *sys.path]):
            import image_secret_gate as gate

            for missing in (None, RULE, packages.RULE):
                with self.subTest(missing=missing):
                    candidate = copy.deepcopy(manifest)
                    candidate["scanner_manifest_sha256"] = gate.MANIFEST_SHA256
                    row = candidate["entries"][0]
                    row["matches"] = [match for match in row["matches"] if match["rule"] != missing]
                    if missing == RULE:
                        del row["self_test"]
                    evidence = json.dumps(candidate).encode()
                    output = self.root / f"executable-scan-{missing}"
                    with patch.object(gate, "load_package_evidence", return_value=(
                            evidence, digest_bytes(evidence), packages.REVIEW)):
                        if missing is None:
                            gate.verified_staging_scan(archive, output, component="runtime")
                            report = json.loads((output / "REPORT.json").read_bytes())
                        else:
                            with self.assertRaisesRegex(gate.ScanRefusal, "remains blocking") as refusal:
                                gate.verified_staging_scan(archive, output, component="runtime")
                            report = refusal.exception.package_report
                    self.assertEqual(report["findings"], raw["findings"])
                    self.assertEqual({entry["finding_id"] for entry in report["package_evidence"]["entries"]},
                                     {identity for rule, identity in findings.items() if rule != missing})
                    self.assertEqual([entry["finding_id"] for entry in report["blocking_findings"]],
                                     [findings[missing]] if missing else [])

    def test_unapproved_key_nonexecutable_member_and_missing_record_stay_blocking(self):
        for option in ({"extra_key": True}, {"mode": 0o644}, {"installed": False}):
            with self.subTest(option=option):
                archive, manifest, raw = self.fixture(**option)
                section, final = self.classify(archive, raw, self.policy(manifest))
                self.assertEqual(section["entries"], [])
                self.assertEqual(final["blocking_findings"], raw["blocking_findings"])

    def test_archive_image_and_finding_identity_are_required(self):
        archive, manifest, raw = self.fixture()
        for key, value in (("archive_sha256", "a" * 64), ("image_config", "sha256:" + "a" * 64),
                           ("image_manifest", "sha256:" + "a" * 64), ("finding_ids", ["a" * 64])):
            with self.subTest(key=key):
                invalid = copy.deepcopy(manifest)
                invalid["entries"][0]["self_test"]["executable"][key] = value
                section, final = self.classify(archive, raw, self.policy(invalid))
                self.assertEqual(section["entries"], [])
                self.assertEqual(final["blocking_findings"], raw["blocking_findings"])
        invalid = copy.deepcopy(manifest)
        invalid["packages"][0]["authority_type"] = "pinned-upstream"
        with self.assertRaises(ScanError):
            self.policy(invalid)
