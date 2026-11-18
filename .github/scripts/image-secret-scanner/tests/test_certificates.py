"""Public CA treatment requires parsed content and the same saved layer state."""

import subprocess
import tarfile
import unittest
from unittest.mock import patch

from image_secrets.certificates import CertificateState, certificate_proof
from image_secrets.common import ScanError
from image_secrets.policy import Findings
from tests import test_scanner as scanner_tests
from tests.fixtures import docker_archive, fake_key, tar_bytes


class CertificateTests(unittest.TestCase):
    setUp = scanner_tests.ScannerTests.setUp
    tearDown = scanner_tests.ScannerTests.tearDown
    scan = scanner_tests.ScannerTests.scan

    def certificate(self, ca=True):
        key, cert = self.root / "ephemeral-key", self.root / "certificate"
        try:
            subprocess.run(
                [
                    "openssl",
                    "req",
                    "-x509",
                    "-newkey",
                    "ed25519",
                    "-nodes",
                    "-days",
                    "1",
                    "-keyout",
                    str(key),
                    "-out",
                    str(cert),
                    "-subj",
                    "/CN=synthetic",
                    "-addext",
                    "basicConstraints=critical,CA:" + ("TRUE" if ca else "FALSE"),
                ],
                check=True,
                capture_output=True,
                timeout=15,
            )
            return cert.read_bytes()
        finally:
            key.unlink(missing_ok=True)
            cert.unlink(missing_ok=True)

    def test_only_ca_certificate_bytes_qualify(self):
        ca = self.certificate()
        self.assertIsNotNone(certificate_proof(ca))
        self.assertIsNotNone(certificate_proof(ca + b"\n" + ca))
        for data in (
            self.certificate(False),
            b"not a certificate",
            ca + fake_key(),
            ca + b"trailing payload",
        ):
            self.assertIsNone(certificate_proof(data))
        with patch("image_secrets.certificates.subprocess.run", side_effect=FileNotFoundError):
            with self.assertRaises(ScanError):
                certificate_proof(ca)

    def state_layer(self, entries, state=None):
        state = state or CertificateState()
        layer = self.root / "layer.tar"
        layer.write_bytes(tar_bytes(entries))
        findings = Findings(None)
        state.layer(layer, 0, 0, findings)
        return state, findings.public_ca

    def test_symlink_resolution_requires_exact_allowed_ca_content(self):
        ca = self.certificate()
        state, proofs = self.state_layer(
            [
                ("usr/share/ca-certificates/root.crt", ca),
                ("etc/ssl/certs/root.pem", (tarfile.SYMTYPE, "/usr/share/ca-certificates/root.crt")),
            ]
        )
        self.assertEqual(len(proofs), 1)
        _, proofs = self.state_layer(
            [
                ("etc/ssl/certs/outside.pem", (tarfile.SYMTYPE, "/tmp/root.crt")),
                ("tmp/root.crt", ca),
                ("etc/ssl/certs/cycle.pem", (tarfile.SYMTYPE, "cycle.pem")),
            ],
            state,
        )
        self.assertEqual(proofs, {})

    def test_ambiguous_hardlinked_and_deleted_entries_cannot_qualify(self):
        ca = self.certificate()
        for entries in (
            [("etc/ssl/certs/root.pem", ca), ("etc/ssl/certs/root.pem", ca)],
            [
                ("etc/ssl/certs/root.crt", ca),
                ("etc/ssl/certs/root.pem", (tarfile.LNKTYPE, "etc/ssl/certs/root.crt")),
            ],
            [("etc/ssl/certs/root.pem", ca), ("etc/ssl/certs/.wh.root.pem", b"")],
        ):
            _, proofs = self.state_layer(entries)
            self.assertEqual(proofs, {})

    def test_later_public_certificate_never_hides_earlier_private_material(self):
        key = fake_key()
        archive = self.root / "image.tar"
        docker_archive(
            archive,
            [
                tar_bytes([("etc/ssl/certs/root.pem", key)]),
                tar_bytes(
                    [("etc/ssl/certs/.wh.root.pem", b""), ("etc/ssl/certs/root.pem", self.certificate())]
                ),
            ],
        )
        report, output = self.scan(archive, 1, ["--blocking-rules=credential", "--public-ca-certificates"])
        self.assertTrue(report["complete"])
        self.assertTrue(
            any(item["rule"] == "content.private-key-header" for item in report["blocking_findings"])
        )
        self.assertNotIn(key.decode(), output)


if __name__ == "__main__":
    unittest.main()
