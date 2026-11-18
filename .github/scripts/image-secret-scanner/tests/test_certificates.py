"""Public CA treatment requires parsed content and the same saved layer state."""

import base64
import gzip
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

    def test_certificate_envelope_must_consume_exactly_one_der_object(self):
        ca = self.certificate()
        der = base64.b64decode(b"".join(ca.splitlines()[1:-1]), validate=True)
        private_der = subprocess.run(
            ["openssl", "genpkey", "-algorithm", "ED25519", "-outform", "DER"],
            check=True,
            capture_output=True,
            timeout=15,
        ).stdout
        for label, tail in (
            ("plain-tail", b"non-certificate payload"),
            ("private-der-tail", private_der),
            ("second-der", der),
            ("compressed-key-tail", gzip.compress(fake_key())),
        ):
            with self.subTest(case=label):
                pem = (
                    b"-----BEGIN CERTIFICATE-----\n"
                    + base64.b64encode(der + tail)
                    + b"\n-----END CERTIFICATE-----\n"
                )
                self.assertIsNone(certificate_proof(pem))
                archive = self.root / "image.tar"
                docker_archive(archive, [tar_bytes([("etc/ssl/certs/review.pem", pem)])])
                report, _ = self.scan(archive, 1, ["--blocking-rules=credential", "--public-ca-certificates"])
                self.assertTrue(report["complete"])
                self.assertFalse(report["public_ca_findings"])
                self.assertTrue(report["blocking_findings"])

    def test_base64_padding_and_framing_are_strict(self):
        ca = self.certificate()
        body = b"".join(ca.splitlines()[1:-1])
        for label, value in (
            ("illegal", body + b"!"),
            ("leading-padding", b"=" + body),
            ("extra-padding", body + b"===="),
            ("missing-byte", body[:-1]),
        ):
            with self.subTest(case=label):
                pem = b"-----BEGIN CERTIFICATE-----\n" + value + b"\n-----END CERTIFICATE-----\n"
                self.assertIsNone(certificate_proof(pem))
        archive = self.root / "image.tar"
        for data in (ca, ca + b"\n" + ca):
            docker_archive(archive, [tar_bytes([("etc/ssl/certs/review.pem", data)])])
            report, _ = self.scan(archive, 0, ["--blocking-rules=credential", "--public-ca-certificates"])
            self.assertTrue(report["complete"])
            self.assertTrue(report["public_ca_findings"])

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

    def test_compatibility_alias_requires_the_same_parsed_ca_bytes(self):
        for data, allowed in ((self.certificate(), True), (fake_key(), False)):
            _, proofs = self.state_layer(
                [
                    ("usr/share/ca-certificates/root.crt", data),
                    ("etc/ssl/cert.pem", (tarfile.SYMTYPE, "/usr/share/ca-certificates/root.crt")),
                    ("etc/ssl1.1/cert.pem", (tarfile.SYMTYPE, "/etc/ssl/cert.pem")),
                ]
            )
            self.assertEqual(len(proofs), 2 if allowed else 0)

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
