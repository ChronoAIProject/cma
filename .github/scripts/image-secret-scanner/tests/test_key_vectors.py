"""The upstream exception cannot transfer to a different package, path or byte."""

import copy
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from image_secrets.common import ScanError
from image_secrets.gate import assess
from image_secrets.key_vectors import (
    PEM,
    package_matches,
    policy,
    prove,
    provenance_bytes,
    reviewed_vectors,
    sha,
)
from tests import test_scanner as scanner_tests
from tests.fixtures import docker_archive, fake_key, tar_bytes


class KeyVectorTests(unittest.TestCase):
    scan = scanner_tests.ScannerTests.scan

    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.counter = 0
        self.expected = copy.deepcopy(policy())
        # Runtime-generated, deliberately nonfunctional blocks exercise the
        # binding algorithm without storing any private-key material in git.
        self.data = b"synthetic-library\x00" + b"\x00".join(fake_key() for _ in range(12))
        self.expected["library"]["sha256"] = sha(self.data)
        self.expected["blocks"] = [
            {"offset": match.start(), "length": len(match[0]), "sha256": sha(match[0])}
            for match in PEM.finditer(self.data)
        ]
        self.provenance = provenance_bytes(self.expected)
        self.status = (
            b"Package: libgnutls30t64\nVersion: 3.8.9-3+deb13u4\nArchitecture: amd64\n"
            b"Status: install ok installed\n\n"
        )
        self.listing = ("/" + self.expected["library"]["path"] + "\n").encode()

    def test_dpkg_empty_fields_preserve_exact_package_validation(self):
        unrelated = b"Package: unrelated\nConffiles:\nDescription: example\n continuation\n\n"
        self.assertTrue(package_matches(unrelated + self.status, self.listing, self.expected))
        for prefix in (
            b"Package: unrelated\nPackage: duplicate\n\n",
            b"malformed field\n\n",
            b" Package: missing-header\nInvalid field: value\n\n",
        ):
            self.assertFalse(package_matches(prefix + self.status, self.listing, self.expected))

    def test_whole_file_all_blocks_and_exact_package_identity_are_required(self):
        proof = prove(self.data, self.status, self.listing, self.provenance, self.expected)
        self.assertIsNotNone(proof)
        for data, status, listing in (
            (self.data + b"changed", self.status, self.listing),
            (self.data, self.status.replace(b"deb13u4", b"deb13u5"), self.listing),
            (self.data, self.status.replace(b"amd64", b"arm64"), self.listing),
            (self.data, self.status + self.status, self.listing),
            (self.data, self.status, b"/somewhere/else\n"),
        ):
            self.assertIsNone(prove(data, status, listing, self.provenance, self.expected))
        self.assertIsNone(prove(self.data, self.status, self.listing, b"wrong-package-digest", self.expected))
        extra = self.data + fake_key()
        expected = dict(self.expected, library=dict(self.expected["library"], sha256=sha(extra)))
        self.assertIsNone(prove(extra, self.status, self.listing, provenance_bytes(expected), expected))

    def report(self):
        proof = prove(self.data, self.status, self.listing, self.provenance, self.expected)
        finding = {
            "finding_id": "1" * 64,
            "scope": "layer",
            "image": 0,
            "layer": 1,
            "entry": 2,
            "path": "path-sha256:" + sha(self.expected["library"]["path"].encode()),
            "rule": "content.private-key-header",
        }
        return {
            "policy": {"upstream_test_vectors_sha256": proof["policy_sha256"]},
            "findings": [finding],
            "allowlisted": [],
            "upstream_test_vectors": [dict(proof, finding_id=finding["finding_id"])],
        }

    def test_proof_cannot_transfer_to_omarchy_nested_members_or_other_paths(self):
        with patch("image_secrets.key_vectors.policy", return_value=self.expected):
            self.assertEqual(reviewed_vectors(self.report(), "runtime"), {"1" * 64})
            for component in ("web", "runtime-omarchy", None):
                with self.assertRaises(ScanError):
                    reviewed_vectors(self.report(), component)
            for changes in (
                {"path": "path-sha256:" + "2" * 64},
                {"member": 0},
                {"rule": "path.ssh-key"},
                {"file_sha256": "3" * 64},
                {"scope": "config"},
            ):
                report = self.report()
                report["findings"][0].update(changes)
                with self.assertRaises(ScanError):
                    reviewed_vectors(report, "runtime")

    def test_only_exact_upstream_proof_can_classify_a_key_finding(self):
        report = self.report()
        report["findings"].append(
            dict(report["findings"][0], finding_id="2" * 64, path="path-sha256:" + "3" * 64)
        )
        with patch("image_secrets.key_vectors.policy", return_value=self.expected):
            assess(report, "credential", component="runtime")
        self.assertEqual(report["counts"]["detected"], 2)
        self.assertEqual(report["counts"]["reviewed"], 1)
        self.assertEqual(report["counts"]["blocking"], 1)
        self.assertEqual(report["blocking_findings"][0]["finding_id"], "2" * 64)

    def test_complete_layer_scan_preserves_twelve_occurrences_and_rejects_copies(self):
        entries = [
            (self.expected["library"]["path"], self.data),
            ("var/lib/dpkg/status", self.status),
            ("var/lib/dpkg/info/libgnutls30t64:amd64.list", self.listing),
            ("usr/share/cma-runtime/package-provenance/gnutls.json", self.provenance),
        ]
        archive = self.root / "image.tar"
        options = ["--blocking-rules=credential", "--reviewed-component=runtime", "--upstream-test-vectors"]
        with patch("image_secrets.key_vectors.policy", return_value=self.expected):
            docker_archive(archive, [tar_bytes(entries)])
            report, _ = self.scan(archive, 0, options)
            self.assertTrue(report["complete"])
            self.assertTrue(report["upstream_test_vectors"])
            self.assertTrue(all(len(proof["occurrences"]) == 12 for proof in report["upstream_test_vectors"]))
            docker_archive(archive, [tar_bytes([*entries, ("tmp/copied-library", self.data)])])
            report, _ = self.scan(archive, 1, options)
            self.assertTrue(
                any(item["rule"] == "content.private-key-header" for item in report["blocking_findings"])
            )
            docker_archive(
                archive, [tar_bytes([("usr/lib", (tarfile.SYMTYPE, "elsewhere"))]), tar_bytes(entries)]
            )
            report, _ = self.scan(archive, 1, options)
            self.assertFalse(report["upstream_test_vectors"])
            docker_archive(archive, [tar_bytes(entries[:-1])])
            report, _ = self.scan(archive, 1, options)
            self.assertFalse(report["upstream_test_vectors"])


if __name__ == "__main__":
    unittest.main()
