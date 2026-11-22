"""Exclude only exactly pinned, re-proved public certificate blocks from the encoded-key guard."""

import base64
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import tarfile

from image_secrets import certificate_blocks, reviewed_literals
from image_secrets.common import ScanError, digest_bytes
from image_secrets.gate import FORMAT_MARKER_ADVISORY
from tests import fixtures
from tests import test_derived_payload as derived_tests
from tests import test_package_policy as packages

HEADER = b"-----BEGIN " + b"PRIVATE KEY-----"
RULE = "content.private-key-header"
# An old-format OpenPGP public-key packet header with an out-of-range length.
PACKET = "99010004"


def openssl(*args, data=None):
    return subprocess.run(["openssl", *args], input=data, capture_output=True, check=True, timeout=20).stdout


def pem(der, label=b"CERTIFICATE"):
    body = base64.b64encode(der)
    lines = b"\n".join(body[index:index + 64] for index in range(0, len(body), 64))
    return b"-----BEGIN " + label + b"-----\n" + lines + b"\n-----END " + label + b"-----"


class CertificateBlockTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="certificate-blocks-")
        self.root = Path(self.scratch.name)
        self.key = self.root / "issuer.pem"
        openssl("genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-out", str(self.key))

    def tearDown(self):
        self.scratch.cleanup()

    def certificate(self):
        # Fix the serial width so each padding byte advances the packet offset.
        # A 64-character PEM line decodes to 48 DER bytes.
        for pad in range(48):
            der = openssl("req", "-x509", "-new", "-key", str(self.key), "-subj", "/CN=fixture", "-days", "1",
                          "-set_serial", "1",
                          "-addext", f"subjectKeyIdentifier={'ab' * pad}{PACKET}aabbccddeeff00112233445566778899",
                          "-outform", "DER")
            if der.find(bytes.fromhex(PACKET)) % 48 == 0:
                return der
        self.fail("no aligned fixture certificate")

    def fixture(self, *, block=None, extra=b""):
        block = pem(self.certificate()) if block is None else block
        data = b"\0" * 32 + HEADER + b"\0" + b"public parser data\0" + block + b"\n\0" + extra + b"\0" * 32
        path = self.root / "member"
        path.write_bytes(data)
        start = data.index(HEADER)
        proof = {"findings": [{"rule": RULE, "spans": [{
            "offset": start, "size": len(HEADER), "sha256": digest_bytes(HEADER),
            "context": {"offset": start - 8, "size": len(HEADER) + 16,
                        "sha256": digest_bytes(data[start - 8:start + len(HEADER) + 8])},
            "source": {"path": "src/fixture.h", "line": 1, "sha256": "a" * 64}}]}]}
        offset = data.index(block)
        blocks = [{"offset": offset, "size": len(block), "sha256": digest_bytes(block)}]
        return path, proof, blocks

    def test_pinned_certificate_lines_skip_only_the_encoded_guard(self):
        path, proof, blocks = self.fixture()
        self.assertFalse(reviewed_literals.encoded_material_absent(path.read_bytes()))
        self.assertEqual(reviewed_literals.verify(path, b"metadata", proof), {})
        self.assertEqual(set(reviewed_literals.verify(path, b"metadata", proof, certificates=blocks)), {RULE})

    def test_alignment_does_not_depend_on_automatic_serial_width(self):
        generate = openssl

        def varying_serial(*args, **kwargs):
            if args[0] != "req" or "-set_serial" in args:
                return generate(*args, **kwargs)
            # Both widths are valid automatic serials. Select the width that
            # moves this padding attempt off a PEM line boundary.
            for serial in (2 ** 144, 2 ** 152):
                der = generate(*args, "-set_serial", str(serial), **kwargs)
                if der.find(bytes.fromhex(PACKET)) % 48 != 0:
                    return der
            self.fail("fixture serial widths did not move the packet")

        with patch(f"{__name__}.openssl", side_effect=varying_serial):
            self.test_pinned_certificate_lines_skip_only_the_encoded_guard()

    def test_changed_block_bytes_or_coordinates_refuse(self):
        path, proof, blocks = self.fixture()
        for name, change in (("digest", {"sha256": digest_bytes(b"other")}), ("offset", {"offset": blocks[0]["offset"] + 1}),
                             ("size", {"size": blocks[0]["size"] - 1})):
            with self.subTest(name=name):
                self.assertEqual(reviewed_literals.verify(path, b"metadata", proof,
                                                          certificates=[dict(blocks[0], **change)]), {})
        data = bytearray(path.read_bytes())
        line = data.index(b"\n", blocks[0]["offset"]) + 5
        data[line] = ord("A") if data[line] != ord("A") else ord("B")
        path.write_bytes(bytes(data))
        self.assertEqual(reviewed_literals.verify(path, b"metadata", proof, certificates=blocks), {})

    def test_a_pinned_block_must_be_certificate_only(self):
        key = openssl("pkey", "-in", str(self.key), "-outform", "DER")
        for name, block in (("private-key-in-certificate-label", pem(key)),
                            ("not-a-certificate", pem(bytes.fromhex("3006020101020102")))):
            with self.subTest(name=name):
                path, proof, blocks = self.fixture(block=block)
                self.assertEqual(reviewed_literals.verify(path, b"metadata", proof, certificates=blocks), {})

    def test_encoded_key_material_outside_the_blocks_still_blocks(self):
        key = openssl("genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-outform", "DER")
        path, proof, blocks = self.fixture(extra=base64.b64encode(key))
        self.assertEqual(reviewed_literals.verify(path, b"metadata", proof, certificates=blocks), {})

    def test_catalog_pin_binds_package_member_and_block_list(self):
        path, proof, blocks = self.fixture()
        row = {"path": "usr/lib/libfixture.so.1", "file_sha256": digest_bytes(path.read_bytes()), "platform": "linux/amd64",
               "member": None, "reviewed_literals": proof, "certificate_blocks": blocks}
        package = {"name": "fixture", "version": "1.0-1", "architecture": "x86_64", "artifact_sha256": "b" * 64,
                   "authority_type": "arch-signature"}
        pin = {key: package[key] for key in ("name", "version", "architecture", "artifact_sha256")}
        pin.update(path=row["path"], file_sha256=row["file_sha256"], blocks=1,
                   blocks_sha256=digest_bytes(certificate_blocks.canonical(blocks)))
        with patch.object(certificate_blocks, "PIN", pin):
            certificate_blocks.validate(row, package)
            for name, (target, change) in {
                "version": (package, {"version": "1.0-2"}), "package": (package, {"name": "other"}),
                "authority": (package, {"authority_type": "pinned-upstream"}),
                "path": (row, {"path": row["path"] + ".other"}), "file": (row, {"file_sha256": "c" * 64}),
                "block": (row, {"certificate_blocks": [dict(blocks[0], offset=blocks[0]["offset"] + 1)]}),
                "count": (row, {"certificate_blocks": blocks * 2}),
            }.items():
                with self.subTest(name=name):
                    changed = {"row": dict(row), "package": dict(package)}
                    changed["package" if target is package else "row"].update(change)
                    with self.assertRaises(ScanError):
                        certificate_blocks.validate(changed["row"], changed["package"])


class CertificateBlockGateTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    detector_scan = packages.PackageTests.detector_scan
    manifest = packages.PackageTests.manifest
    publish = derived_tests.DerivedPayloadTests.publish

    def test_whole_gate_applies_the_pinned_blocks_to_the_marker_row(self):
        helper = CertificateBlockTests()
        helper.root, helper.key = self.root, self.root / "issuer.pem"
        openssl("genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-out", str(helper.key))
        member, proof, blocks = helper.fixture()
        contents = member.read_bytes()
        path = "usr/lib/libfixture.so.1"
        supplied = fixtures.tar_bytes([(path, contents)])
        layer = fixtures.tar_bytes([(path, contents), ("etc/machine-id", b""),
                                    ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id"))])
        archive = self.root / "certificate-blocks.tar"
        fixtures.oci_archive(archive, [layer])
        raw = self.detector_scan(archive, "runtime-omarchy")
        [image] = raw["inventory"]["images"]
        [marker] = [row for row in raw["blocking_findings"] if row["rule"] == RULE]
        manifest = self.manifest(supplied, layer, component="runtime-omarchy", file=path)
        manifest["packages"][0].update(name="fixture", version="1.0-1", authority_type="arch-signature",
                                       architecture="x86_64")
        [row] = manifest["entries"]
        identity = {"archive_sha256": raw["archive_sha256"], "image_config": image["config_digest"],
                    "image_manifest": image["manifest"]}
        [finding] = proof["findings"]
        row["reviewed_literals"] = {**identity, "findings": [dict(finding, finding_id=marker["finding_id"])]}
        row["matches"] = [{"rule": RULE, "candidate_sha256": None, "line": None, "explanation": FORMAT_MARKER_ADVISORY,
                           "proof_sha256": "a" * 64}]
        package = manifest["packages"][0]
        pin = {key: package[key] for key in ("name", "version", "architecture", "artifact_sha256")}
        pin.update(path=path, file_sha256=row["file_sha256"], blocks=1,
                   blocks_sha256=digest_bytes(certificate_blocks.canonical(blocks)))
        with patch.object(certificate_blocks, "PIN", pin):
            refused = self.publish(archive, manifest, "without-blocks", blocks=True)
            self.assertEqual([item["rule"] for item in refused["blocking_findings"]], [RULE])
            row["certificate_blocks"] = blocks
            clean = self.publish(archive, manifest, "with-blocks")
            self.assertEqual(clean["findings"], raw["findings"])
            self.assertEqual(clean["blocking_findings"], [])


if __name__ == "__main__":
    unittest.main()
