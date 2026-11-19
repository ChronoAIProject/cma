"""Only complete, actual archive-bound X.509 objects qualify as public material."""

import base64
import subprocess
import tarfile
import unittest
from unittest.mock import patch

from image_secrets.common import ScanError, digest_bytes
from image_secrets.key_material import has_binary_key
from image_secrets.trust_store import certificate_digest
from tests import test_scanner as scanner_tests
from tests.fixtures import docker_archive, tar_bytes
from tests.test_review_regressions import token


class TrustStoreTests(unittest.TestCase):
    setUp = scanner_tests.ScannerTests.setUp
    tearDown = scanner_tests.ScannerTests.tearDown
    scan = scanner_tests.ScannerTests.scan

    def material(self):
        # Generate a disposable test identity. Only the public certificate and
        # boolean detector results can appear in test output.
        key = self.root / "test-key"
        def run(*args, data=None):
            return subprocess.run(["openssl", *args], input=data, capture_output=True,
                                  check=True, timeout=15).stdout
        pem = run("req", "-x509", "-newkey", "rsa:2048", "-keyout", str(key), "-nodes",
                  "-days", "1", "-subj", "/CN=fixture.invalid")
        der = run("x509", "-outform", "DER", data=pem)
        pkcs8 = run("pkcs8", "-topk8", "-nocrypt", "-outform", "DER", "-in", str(key))
        encrypted = run("pkcs8", "-topk8", "-outform", "DER", "-in", str(key), "-passout", "pass:fixture")
        return pem, der, key.read_bytes(), pkcs8, encrypted

    def image_scan(self, layers, expected=1, mode="credential", component="runtime"):
        archive = self.root / "image.tar"
        docker_archive(archive, [tar_bytes(layer) for layer in layers])
        options = ["--blocking-rules", mode]
        if component and mode == "credential":
            options += ["--staging-runtime-component", component]
        return self.scan(archive, expected, options)[0]

    def test_certificate_bundles_der_and_relative_or_absolute_link_chains(self):
        pem, der, *_ = self.material()
        target = "usr/share/ca-certificates/vendor/root.crt"
        folder = "etc/ssl/certs/"
        first = [(target, pem + b"\n" + pem), (folder + "outside-named.pem", der)]
        second = [(folder + "alias.pem", (tarfile.SYMTYPE, "../../../" + target)),
                  (folder + "chain.pem", (tarfile.SYMTYPE, "/" + folder + "alias.pem"))]
        report = self.image_scan([first, second], 0)
        public = [row for row in report["advisory_findings"] if row["rule"] == "path.key-or-certificate"]
        self.assertEqual(len(public), 3)
        self.assertEqual({row["classification"] for row in public}, {"public-material"})
        self.assertEqual({row["public_certificate_sha256"] for row in public},
                         {digest_bytes(der), digest_bytes(pem + b"\n" + pem)})
        strict = self.image_scan([first, second], 1, "all")
        self.assertEqual(sum(row["rule"] == "path.key-or-certificate" for row in strict["blocking_findings"]), 3)

    def test_only_explicit_staging_runtime_scopes_enable_certificate_advisories(self):
        pem, *_ = self.material()
        layers = [[("etc/pki/root.pem", pem)]]
        for component in ("runtime", "runtime-omarchy", None):
            report = self.image_scan(layers, 0 if component else 1, component=component)
            self.assertEqual(report["policy"].get("staging_runtime_component"), component)
            self.assertEqual(sum("public_certificate_sha256" in row for row in report["findings"]),
                             int(component is not None))

    def test_pem_and_der_apply_the_same_credential_detectors_to_certificate_fields_and_aliases(self):
        pem, *_ = self.material()
        ext = self.root / "extension.cnf"
        ext.write_text("[ext]\nsubjectAltName=URI:https://fixture:synthetic@demo.invalid/" + token() + "/\n")
        signed = subprocess.run(["openssl", "x509", "-signkey", str(self.root / "test-key"), "-extfile",
                                 str(ext), "-extensions", "ext"], input=pem, capture_output=True,
                                check=True, timeout=15).stdout
        der = subprocess.run(["openssl", "x509", "-outform", "DER"], input=signed, capture_output=True,
                             check=True, timeout=15).stdout
        for value in (signed, der):
            rows = [("etc/pki/leaf.pem", value), ("etc/pki/alias.pem", (tarfile.SYMTYPE, "./leaf.pem"))]
            report = self.image_scan([rows])
            rules = {row["rule"] for row in report["blocking_findings"]}
            self.assertTrue({"content.credential-url", "gitleaks.github-pat"} <= rules)
            self.assertEqual(sum(row["rule"] == "path.key-or-certificate" for row in report["blocking_findings"]), 2)
            self.assertFalse(any("public_certificate_sha256" in row for row in report["findings"]))

    def test_decoded_inspection_must_complete_for_a_certificate_receipt(self):
        pem, *_ = self.material()
        with patch("image_secrets.engine.Stage.certificate", side_effect=ScanError("detector-unavailable")):
            report = self.image_scan([[("etc/pki/leaf.pem", pem)]], expected=2)
        self.assertFalse(report["complete"])
        self.assertEqual(report["error"], "detector-unavailable")
        self.assertFalse(any("public_certificate_sha256" in row for row in report["findings"]))

    def test_mixed_unparseable_malformed_and_outside_material_block(self):
        pem, der, key, pkcs8, _ = self.material()
        invalid = [pem + key, pem + pkcs8, der + pkcs8, key + pem, pem + b"trailing",
                   der + b"\x00", pem + pem[:-9], b"-----BEGIN CERTIFICATE-----\nMAA=\n-----END CERTIFICATE-----",
                   b"\x30\x04test", b"", b"prefix" + pem]
        rows = [(f"etc/pki/c{idx}.pem", value) for idx, value in enumerate(invalid)]
        rows += [("untrusted/good.pem", pem), ("etc/ssl/certs-neighbor/good.pem", pem)]
        report = self.image_scan([rows])
        self.assertEqual(sum(row["rule"] == "path.key-or-certificate" for row in report["blocking_findings"]),
                         len(rows))
        self.assertFalse(any("public_certificate_sha256" in row for row in report["findings"]))
        self.assertIn("content.private-key-header", {row["rule"] for row in report["blocking_findings"]})

    def test_binary_keys_and_openssh_block_without_a_key_filename(self):
        _, der, _, pkcs8, encrypted = self.material()
        ssh = self.root / "ephemeral"
        subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(ssh)], check=True,
                       capture_output=True, timeout=15)
        ssh_pem = ssh.read_bytes()
        ssh_raw = base64.b64decode(b"".join(ssh_pem.splitlines()[1:-1]), validate=True)
        self.assertFalse(has_binary_key(b"header openssh-key-v1\x00 footer"))
        ec_pem = subprocess.run(["openssl", "ecparam", "-name", "prime256v1", "-genkey", "-noout"],
                                capture_output=True, check=True, timeout=15).stdout
        ec_der = subprocess.run(["openssl", "ec", "-outform", "DER"], input=ec_pem,
                                capture_output=True, check=True, timeout=15).stdout
        rsa_der = subprocess.run(["openssl", "rsa", "-in", str(self.root / "test-key"), "-outform", "DER"],
                                 capture_output=True, check=True, timeout=15).stdout
        rows = [("app/data1", pkcs8), ("app/data2", encrypted), ("app/data3", ssh_pem),
                ("app/data4", ssh_raw), ("app/data5", b"\0" * (1024 * 1024 - 3) + pkcs8),
                ("app/data6", der + pkcs8), ("app/data7", ec_der), ("app/data8", rsa_der)]
        report = self.image_scan([rows])
        blocked = {(r["entry"], r["rule"]) for r in report["blocking_findings"]}
        for i in (0, 1, 3, 4, 5, 6, 7):
            self.assertIn((i, "content.binary-private-key"), blocked)
        self.assertIn((2, "content.private-key-header"), blocked)

    def test_symlinks_refuse_missing_unsafe_replaced_or_ambiguous_targets(self):
        pem, *_ = self.material()
        root = "etc/ssl/certs/"
        def link(target):
            return tarfile.SYMTYPE, target
        first = [(root + "removed.crt", pem), (root + "future.pem", link("future.crt")),
                 (root + "removed.pem", link("removed.crt")), (root + "swap.crt", pem),
                 (root + "unsafe.pem", link("dir/../safe.crt")), (root + "safe.crt", pem),
                 (root + "missing.pem", link("absent")), (root + "escape.pem", link("../../../../app/cert")),
                 (root + "cycle.pem", link("cycle.pem")), (root + "foreign.pem", link("/app/public.crt")),
                 ("app/public.crt", pem), (root + "swap.pem", link("swap.crt")),
                 (root + "dot.pem", link("safe.crt/.")),
                 (root + "sub", link("/app")), (root + "sub/misleading.pem", pem),
                 (root + "hard.pem", (tarfile.LNKTYPE, root + "safe.crt"))]
        second = [(root + "future.crt", pem), (root + ".wh.removed.crt", b""),
                  (root + "swap.crt", b"bad")]
        report = self.image_scan([first, second])
        self.assertEqual(sum(row["rule"] == "path.key-or-certificate" for row in report["blocking_findings"]), 11)
        self.assertFalse(any("public_certificate_sha256" in row for row in report["findings"]))

    def test_key_in_certificate_extension_and_der_length_mismatches_are_refused(self):
        pem, der, _, key, _ = self.material()
        f = self.root / "candidate"
        for bad in (der[:-1], der + key, pem + b"\x00", b"\x30\x80\x00\x00"):
            f.write_bytes(bad)
            self.assertIsNone(certificate_digest(f, lambda _data: None))
        ext = self.root / "extension.cnf"
        ext.write_text("[ext]\n1.2.3.4=DER:" + key.hex() + "\n")
        public = subprocess.run(["openssl", "x509", "-in", "/dev/stdin", "-signkey", str(self.root / "test-key"),
                                 "-extfile", str(ext), "-extensions", "ext"], input=pem,
                                capture_output=True, check=True, timeout=15).stdout
        f.write_bytes(public)
        self.assertIsNone(certificate_digest(f, lambda _data: None))
