"""Exercise exact artifact decisions through complete layer and container scans."""

import base64
import io
import json
import subprocess
import tarfile
import unittest
import zipfile
from unittest.mock import patch

from image_secrets.key_vectors import sha
from image_secrets.policy import PRIVATE
from tests import test_scanner as scanner_tests
from tests.fixtures import docker_archive, tar_bytes
from tests.test_omarchy_artifacts import PATH, fixture


class OmarchyArtifactIntegrationTests(unittest.TestCase):
    setUp = scanner_tests.ScannerTests.setUp
    tearDown = scanner_tests.ScannerTests.tearDown
    scan = scanner_tests.ScannerTests.scan

    def fixture(self, nested=False):
        data = b"parser\x00-----BEGIN " + b"PRIVATE KEY-----\x00format-only\n"
        entry, state, _finding = fixture(data)
        entry["match"] = {
            "rule": "content.private-key-header",
            "contexts": [
                {
                    "offset": m.start(),
                    "bytes": len(m[0]),
                    "sha256": sha(m[0]),
                    "context_sha256": sha(data[max(0, m.start() - 64) : m.end() + 64]),
                }
                for m in PRIVATE.finditer(data)
            ],
        }
        entry["classification"] = "reviewed-non-key-marker"
        if nested:
            output = io.BytesIO()
            with zipfile.ZipFile(output, "w") as archive:
                archive.writestr("parser.class", data)
            data = output.getvalue()
            entry["path"] += "!parser.class"
            entry["member"] = "0"
            entry["outer"].update(sha256=sha(data), bytes=len(data))
        return entry, [(PATH, data), *state.items()]

    def check(self, entry, files, expected=1, earlier=None, component="runtime-omarchy", extra=()):
        archive = self.root / "image.tar"
        layers = [tar_bytes(files)]
        if earlier is not None:
            layers.insert(0, tar_bytes(earlier))
        docker_archive(archive, layers, {"architecture": "amd64"})
        with (
            patch("image_secrets.omarchy_state.policy", return_value=[entry]),
            patch("image_secrets.omarchy_policy.policy", return_value=[entry]),
        ):
            return self.scan(
                archive,
                expected,
                [
                    "--blocking-rules=credential",
                    "--upstream-test-vectors",
                    "--reviewed-component=" + component,
                    *extra,
                ],
            )[0]

    def test_exact_marker_and_archive_preserve_raw_findings(self):
        entry, files = self.fixture()
        result = self.check(entry, files, 0)
        self.assertTrue(result["complete"])
        self.assertEqual(len(result["reviewed_omarchy_artifacts"]), 1)
        self.assertTrue(result["findings"])
        # ZIP bytes also contain the raw marker. Its outer finding remains blocked;
        # an approved member never grants an implicit outer archive exception.
        entry, files = self.fixture(True)
        result = self.check(entry, files)
        self.assertEqual(len(result["reviewed_omarchy_artifacts"]), 1)
        self.assertTrue(result["blocking_findings"])

    def test_copies_metadata_changes_duplicates_and_earlier_layers_block(self):
        entry, files = self.fixture()
        for case in (
            "copy",
            "changed",
            "missing",
            "version",
            "ownership",
            "duplicate",
            "conflict",
            "ancestor",
            "earlier",
            "component",
            "whiteout",
        ):
            with self.subTest(case=case):
                altered = list(files)
                earlier = None
                if case == "copy":
                    altered.append(("tmp/copy", files[0][1]))
                if case == "changed":
                    altered[0] = (PATH, files[0][1] + b"additional")
                if case == "missing":
                    altered.pop()
                if case == "version":
                    altered[1] = (files[1][0], files[1][1].replace(b"1.2-1", b"1.3-1"))
                if case == "ownership":
                    altered[2] = (files[2][0], b"%FILES%\nother\n")
                if case == "duplicate":
                    altered.append(files[1])
                if case == "conflict":
                    altered.append(("var/lib/pacman/local/example-other/desc", files[1][1]))
                if case == "ancestor":
                    earlier = [("usr/lib", (tarfile.SYMTYPE, "elsewhere"))]
                if case == "earlier":
                    earlier = [("tmp/earlier", files[0][1])]
                if case == "whiteout":
                    altered.append(("var/lib/pacman/local/.wh.example-1.2-1", b""))
                result = self.check(
                    entry,
                    altered,
                    earlier=earlier,
                    component="runtime" if case == "component" else "runtime-omarchy",
                )
                self.assertTrue(result["blocking_findings"])

    def test_real_keys_and_encoded_material_cannot_reuse_marker(self):
        entry, files = self.fixture()
        key = subprocess.run(
            ["openssl", "genpkey", "-algorithm", "ED25519"], check=True, capture_output=True, timeout=15
        ).stdout
        der = subprocess.run(
            ["openssl", "pkey", "-outform", "DER"], input=key, check=True, capture_output=True, timeout=15
        ).stdout
        encrypted = subprocess.run(
            ["openssl", "pkey", "-aes-256-cbc", "-passout", "pass:synthetic"],
            input=key,
            check=True,
            capture_output=True,
            timeout=15,
        ).stdout
        for tail in (
            key,
            b"\x00" + key,
            der,
            encrypted,
            base64.b64encode(key),
            json.dumps(key.decode()).encode(),
        ):
            result = self.check(entry, [(PATH, files[0][1] + tail), *files[1:]])
            self.assertFalse(result["reviewed_omarchy_artifacts"])
            self.assertTrue(result["blocking_findings"])

    def test_changed_java_or_wheel_member_and_extra_occurrence_fail(self):
        entry, files = self.fixture(True)
        for case in ("member", "copied-member", "extra-occurrence"):
            with self.subTest(case=case):
                with zipfile.ZipFile(io.BytesIO(files[0][1])) as archive:
                    body = archive.read("parser.class")
                output = io.BytesIO()
                with zipfile.ZipFile(output, "w") as archive:
                    archive.writestr(
                        "copied.class" if case == "copied-member" else "parser.class",
                        body + (body if case == "extra-occurrence" else b"changed"),
                    )
                result = self.check(entry, [(PATH, output.getvalue()), *files[1:]])
                self.assertFalse(result["reviewed_omarchy_artifacts"])
                self.assertTrue(result["blocking_findings"])

    def test_exact_certificate_comments_never_hide_key_or_malformed_blocks(self):
        from image_secrets.certificates import PEM, certificate_proof
        from image_secrets.omarchy_proofs import certificate_matches
        from tests.test_certificates import CertificateTests

        data = CertificateTests.certificate(self)
        proof = certificate_proof(data)
        expected = {
            "empty": False,
            "der_sha256": proof["certificates"],
            "pem_sha256": [sha(m[0]) for m in PEM.finditer(data)],
        }
        self.assertTrue(certificate_matches(b"# public roots\n" + data, expected))
        key = subprocess.run(
            ["openssl", "genpkey", "-algorithm", "ED25519"], capture_output=True, check=True, timeout=15
        ).stdout
        for changed in (data + key, data + b"\0" + key, data[:-20], data + b"trailing", data + data):
            self.assertFalse(certificate_matches(changed, expected))

    def test_incomplete_scan_cannot_be_clean(self):
        entry, files = self.fixture()
        result = self.check(entry, files, 2, extra=["--max-entries=1"])
        self.assertFalse(result["complete"])
        self.assertNotEqual(result["status"], "CLEAN")


if __name__ == "__main__":
    unittest.main()
