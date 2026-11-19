"""Verify the narrow stage-scoped library vector decision end to end."""

import base64
import copy
import gzip
import hashlib
import json
import subprocess
import sys
import tarfile
import unittest
from pathlib import Path
from unittest.mock import patch

from image_secrets.common import ScanError, digest_bytes
from image_secrets.gate import SELF_TEST_ADVISORY, SELF_TEST_RULES
from image_secrets.library_vectors import complete_pem
from tests import fixtures, test_package_policy as packages

LIBRARY = "usr/lib/libexample.so.1"
DEBIAN_DATABASE = "var/lib/dpkg/info/libexample:amd64.md5sums"
ARCH_DATABASE = "var/lib/pacman/local/libexample-1.0/mtree"
# A non-executable ELF header is sufficient for these data fixtures.
ELF = b"\x7fELF\x02\x01" + bytes(10) + b"\x03\x00" + bytes(46)


class LibraryVectorTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    manifest = packages.PackageTests.manifest
    policy = staticmethod(packages.PackageTests.policy)
    classify = packages.PackageTests.classify
    detector_scan = packages.PackageTests.detector_scan

    def fixture(self, vector, *, component="runtime", kind="pem", unproved=b"", path=LIBRARY,
                later=(), db_error=None):
        payload = ELF + vector + unproved
        arch = component == "runtime-omarchy"
        db_path = ARCH_DATABASE if arch else DEBIAN_DATABASE
        member_path = "usr/lib/libother.so.1" if db_error == "missing-member" else path
        member_checksum = digest_bytes(payload) if arch else hashlib.md5(payload, usedforsecurity=False).hexdigest()
        record_checksum = "0" * len(member_checksum) if db_error == "wrong-checksum" else member_checksum
        if arch:
            database = gzip.compress(f"#mtree\n/set type=file uid=0\n./{member_path} sha256digest={record_checksum}\n"
                                     .encode(), mtime=0)
        else:
            database = f"{record_checksum}  {member_path}\n".encode()
        base = [(path, payload), (db_path, database), ("etc/machine-id", b""),
                ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id"))]
        tar = fixtures.tar_bytes(base)
        supplied = fixtures.tar_bytes([(path, payload)])
        image = self.root / "image.tar"
        fixtures.docker_archive(image, [tar, *later])
        evidence = self.manifest(supplied, tar, component=component, file=path)
        package = evidence["packages"][0]
        package.update(name="libexample", authority_type="arch-signature" if arch else "debian-index",
                       architecture="x86_64" if arch else "amd64", version="1.0")
        row = evidence["entries"][0]
        row["self_test"] = {
            "database": {"path": db_path, "layer": 0, "diff_id": row["diff_id"], "entry": 1,
                         "sha256": digest_bytes(database), "proof_sha256": "3" * 64,
                         "member_checksum": member_checksum},
            "upstream": {"name": "example", "version": "1.0", "archive_sha256": "4" * 64,
                         "authority_sha256": "5" * 64},
            "vectors": [{"offset": len(ELF), "size": len(vector), "sha256": digest_bytes(vector), "kind": kind,
                         "source_member": "lib/self-test.c", "source_line": 12, "self_test_symbol": "signing_self_test"}],
        }
        return image, evidence

    def matches(self, report, evidence):
        span = evidence["entries"][0]["self_test"]["vectors"][0]
        rules = []
        for row in report["findings"]:
            if row["scope"] == "layer" and row["layer"] == 0 and row["entry"] == 0 and row["rule"] in SELF_TEST_RULES:
                digest = row.get("candidate_sha256")
                if row["rule"] == "gitleaks.private-key" and digest != span["sha256"]:
                    continue
                if ((row["rule"] == "content.binary-private-key") != (span["kind"] == "binary")):
                    continue
                rules.append({"rule": row["rule"], "candidate_sha256": digest, "line": row.get("line"),
                              "explanation": SELF_TEST_ADVISORY, "proof_sha256": "6" * 64})
        self.assertTrue(rules)
        evidence["entries"][0]["matches"] = rules

    @staticmethod
    def generated_key(*args):
        return subprocess.run(["openssl", "genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048",
                               *args], check=True, capture_output=True, timeout=30).stdout

    def test_authenticated_pem_is_counted_once_in_both_cache_scopes(self):
        vector = self.generated_key().rstrip(b"\n")
        for component in ("runtime", "runtime-omarchy"):
            with self.subTest(component=component):
                image, evidence = self.fixture(vector, component=component)
                cache = self.root / "layer-cache.json"
                options = ["--detector-cache-context", "d" * 64, "--detector-cache-output", str(cache)]
                raw = self.detector_scan(image, component, options)
                self.matches(raw, evidence)
                verified = self.policy(evidence)
                section, assessed = self.classify(image, raw, verified, component)
                rules = {row["rule"] for row in section["entries"]}
                self.assertEqual(rules, {"content.private-key-header", "gitleaks.private-key"})
                self.assertEqual(assessed["blocking_findings"], [])
                self.assertEqual(assessed["reviewed_classifications"], [])
                self.assertEqual(assessed["counts"]["advisory"], len(raw["findings"]))
                for item in section["entries"]:
                    proof = item["library_self_test"]
                    self.assertEqual(proof["library_path"], LIBRARY)
                    self.assertEqual(proof["vectors"][0]["sha256"], digest_bytes(vector))
                    self.assertEqual(proof["upstream"]["archive_sha256"], "4" * 64)
                cached = self.detector_scan(image, component, [*options, "--detector-cache-input", str(cache)])
                self.assertEqual(cached["detector_cache"]["hits"], 1)
                second, warm = self.classify(image, cached, verified, component)
                self.assertEqual(section, second)
                self.assertEqual(assessed["counts"], warm["counts"])

    def test_complete_binary_vectors_are_required_for_the_aggregate_rule(self):
        vector = self.generated_key("-outform", "DER")
        for extra in (b"", bytes(16) + self.generated_key("-outform", "DER")):
            with self.subTest(has_unproved_key=bool(extra)):
                image, evidence = self.fixture(vector, kind="binary", unproved=extra)
                raw = self.detector_scan(image)
                self.matches(raw, evidence)
                section, assessed = self.classify(image, raw, self.policy(evidence))
                self.assertEqual(bool(section["entries"]), not extra)
                self.assertEqual("content.binary-private-key" in {f["rule"] for f in assessed["blocking_findings"]},
                                 bool(extra))

    def test_modified_provenance_or_nonlibrary_members_refuse(self):
        image, evidence = self.fixture(self.generated_key().rstrip(b"\n"))
        raw = self.detector_scan(image)
        self.matches(raw, evidence)
        for field in ("database", "library", "vector"):
            with self.subTest(field=field):
                bad = copy.deepcopy(evidence)
                row = bad["entries"][0]
                if field == "database":
                    row["self_test"]["database"]["sha256"] = "a" * 64
                elif field == "library":
                    row["package_member"]["sha256"] = row["file_sha256"] = "b" * 64
                else:
                    row["self_test"]["vectors"][0]["offset"] += 1
                section, final = self.classify(image, raw, self.policy(bad))
                self.assertEqual(section["entries"], [])
                self.assertEqual(final["status"], "FINDINGS")

        original = digest_bytes(json.dumps(evidence).encode())
        changed = copy.deepcopy(evidence)
        changed["entries"][0]["self_test"]["upstream"]["archive_sha256"] = "d" * 64
        with self.assertRaises(ScanError):
            self.policy(changed, expected=original)

        wrong_package = copy.deepcopy(evidence)
        wrong_package["packages"][0]["name"] = "another-package"
        with self.assertRaises(ScanError):
            self.policy(wrong_package)

        _, standalone = self.fixture(self.generated_key().rstrip(b"\n"), path="etc/pacman.d/gnupg/secring.gpg")
        with self.assertRaises(ScanError):
            self.policy(standalone)

    def test_native_database_must_record_library_bytes(self):
        vector = self.generated_key().rstrip(b"\n")
        for component in ("runtime", "runtime-omarchy"):
            for problem in ("missing-member", "wrong-checksum"):
                with self.subTest(component=component, problem=problem):
                    image, evidence = self.fixture(vector, component=component, db_error=problem)
                    raw = self.detector_scan(image, component)
                    self.matches(raw, evidence)
                    section, assessed = self.classify(image, raw, self.policy(evidence), component)
                    self.assertEqual(section["entries"], [])
                    self.assertEqual(assessed["status"], "FINDINGS")
                    self.assertEqual({r["rule"] for r in assessed["blocking_findings"]},
                                     {"content.private-key-header", "gitleaks.private-key"})
        image, evidence = self.fixture(vector, db_error="wrong-checksum")
        raw = self.detector_scan(image)
        self.matches(raw, evidence)
        # Even if the reviewed bytes and a syntactically valid record agree,
        # dpkg's digest must still match the actual library.
        evidence["entries"][0]["self_test"]["database"]["member_checksum"] = "0" * 32
        section, assessed = self.classify(image, raw, self.policy(evidence))
        self.assertEqual(section["entries"], [])
        self.assertEqual(assessed["status"], "FINDINGS")

    def test_inline_ec_self_test_requires_a_complete_key(self):
        pem = subprocess.run(["openssl", "ecparam", "-name", "prime256v1", "-genkey", "-noout"],
                             check=True, capture_output=True, timeout=30).stdout
        header = b"-----BEGIN EC PRIVATE KEY-----"
        footer = b"-----END EC PRIVATE KEY-----"
        inline = pem.strip().replace(header + b"\n", header, 1)
        self.assertNotEqual(inline, pem.strip())
        der = base64.b64decode(b"".join(inline[len(header):-len(footer)].splitlines()), validate=True)
        for corrupt in (der[:-1], der + b"trailing"):
            self.assertFalse(complete_pem(header + base64.b64encode(corrupt) + b"\n" + footer))
        image, evidence = self.fixture(inline, component="runtime-omarchy")
        raw = self.detector_scan(image, "runtime-omarchy")
        self.matches(raw, evidence)
        section, assessed = self.classify(image, raw, self.policy(evidence), "runtime-omarchy")
        self.assertIn("content.private-key-header", {item["rule"] for item in section["entries"]})
        self.assertEqual(assessed["blocking_findings"], [])

    def test_unproved_header_and_app_owned_copy_remain_blocking(self):
        vector = self.generated_key().rstrip(b"\n")
        for extra, duplicate in ((b"", True), (b"\n" + self.generated_key().rstrip(b"\n"), False)):
            with self.subTest(duplicate=duplicate):
                app = fixtures.tar_bytes([(LIBRARY, ELF + vector), ("app-owned", b"marker")])
                later = [app] if duplicate else []
                image, evidence = self.fixture(vector, unproved=extra, later=later)
                raw = self.detector_scan(image)
                self.matches(raw, evidence)
                section, assessed = self.classify(image, raw, self.policy(evidence))
                self.assertEqual(bool(section["entries"]), duplicate)
                self.assertTrue(any(row["layer"] == int(duplicate) and row["rule"] == "content.private-key-header"
                                    for row in assessed["blocking_findings"]))

    def test_publication_gate_checks_vector_receipts(self):
        image, evidence = self.fixture(self.generated_key().rstrip(b"\n"))
        self.matches(self.detector_scan(image), evidence)
        scripts = Path(__file__).resolve().parents[2]
        with patch.object(sys, "path", [str(scripts), *sys.path]):
            import image_secret_gate as gate

            evidence["scanner_manifest_sha256"] = gate.MANIFEST_SHA256
            data = json.dumps(evidence).encode()
            output = self.root / "publication"
            with patch.object(gate, "load_package_evidence", return_value=(data, digest_bytes(data), packages.REVIEW)):
                archive, scanner = gate.verified_staging_scan(image, output, component="runtime")
            self.assertEqual(scanner[1], gate.MANIFEST_SHA256)
            report = json.loads((output / "REPORT.json").read_bytes())
            section = report["package_evidence"]
            expected = copy.deepcopy(section)
            self.assertEqual(len(section["entries"]), 2)
            section["entries"][0]["library_self_test"]["vectors"].clear()
            with self.assertRaises(gate.ScanRefusal):
                gate.validate_receipt(report, archive, "credential", None, component="runtime",
                                      package_section=expected)
