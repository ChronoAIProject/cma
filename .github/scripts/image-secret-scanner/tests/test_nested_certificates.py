"""Exercise the exact nested certificate decision through the publication gate."""

import base64
import copy
import io
import json
import stat
import subprocess
import sys
import tarfile
import tempfile
import textwrap
import unittest
import warnings
import zipfile
from pathlib import Path
from unittest.mock import patch

from image_secrets import nested_certificates
from image_secrets.arch_package_ownership import ROOT, VERSION
from image_secrets.common import ScanError, digest_bytes, digest_file
from image_secrets.gate import CERTIFICATE_ADVISORY, CERTIFICATE_RULE
from image_secrets.package_evidence import OMARCHY_PORTABLE_SCHEMA
from tests import fixtures
from tests import test_package_policy as packages

COMPONENT = nested_certificates.COMPONENT
WHEEL = nested_certificates.WHEEL
MEMBER = nested_certificates.MEMBER
SCRIPTS = Path(__file__).resolve().parents[2]


def openssl(*args):
    return subprocess.run(["openssl", *args], timeout=10, check=True, capture_output=True).stdout


def pem(der):
    lines = textwrap.fill(base64.b64encode(der).decode("ascii"), width=64).encode("ascii")
    return b"-----BEGIN CERTIFICATE-----\n" + lines + b"\n-----END CERTIFICATE-----\n"


def wheel_bytes(bundle, *, mode=stat.S_IFREG, duplicate=False, sibling=b""):
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as wheel:
        member = zipfile.ZipInfo(MEMBER)
        member.create_system = 3
        member.external_attr = (mode | 0o644) << 16
        wheel.writestr(member, bundle)
        if duplicate:
            with warnings.catch_warnings():
                warnings.simplefilter("ignore", UserWarning)
                wheel.writestr(copy.copy(member), bundle)
        if sibling:
            wheel.writestr("pip/fixture.py", sibling)
    return output.getvalue()


class NestedCertificateTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    manifest = packages.PackageTests.manifest
    policy = staticmethod(packages.PackageTests.policy)
    classify = packages.PackageTests.classify
    detector_scan = packages.PackageTests.detector_scan

    @classmethod
    def setUpClass(cls):
        with tempfile.TemporaryDirectory(prefix="scanner-cert-fixtures-") as temporary:
            root = Path(temporary)
            key, request, extension = root / "key", root / "request", root / "extensions"
            openssl("req", "-new", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1",
                    "-nodes", "-subj", "/CN=Synthetic CA", "-keyout", str(key), "-out", str(request))
            sign = ("x509", "-req", "-in", str(request), "-signkey", str(key), "-days", "1", "-outform", "DER")
            cls.ders = [openssl(*sign, "-set_serial", str(serial))
                        for serial in range(1, nested_certificates.COUNT + 1)]
            extension.write_text("1.2.3.4=ASN1:UTF8String:-----BEGIN PRIVATE KEY-----\n")
            cls.private_der = openssl(*sign, "-set_serial", "1000", "-extfile", str(extension))
        cls.bundle = b"".join(b"\n# Synthetic CA\n\n" + pem(der) for der in cls.ders)

    def fixture(self, *, bundle=None, ders=None, wheel_options=None):
        bundle = self.bundle if bundle is None else bundle
        wheel = wheel_bytes(bundle, **(wheel_options or {}))
        vendor = fixtures.tar_bytes([(WHEEL, wheel)])
        layer = fixtures.tar_bytes([(WHEEL, wheel), ("etc/machine-id", b""),
                                    ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id"))])
        archive = self.root / "image.tar"
        fixtures.oci_archive(archive, [layer], compress=False)
        raw = self.detector_scan(archive, COMPONENT)
        image = raw["inventory"]["images"][0]
        target = next(row for row in raw["findings"] if row["rule"] == CERTIFICATE_RULE and row["member"] == "0")
        member = {"locator": "0", "path": [MEMBER], "sha256": digest_bytes(bundle)}
        manifest = self.manifest(vendor, layer, component=COMPONENT, file=WHEEL, member=member)
        manifest["packages"][0].update(name="python", authority_type="arch-signature", architecture="x86_64")
        row = manifest["entries"][0]
        row["matches"] = [{"rule": CERTIFICATE_RULE, "candidate_sha256": member["sha256"], "line": None,
                           "explanation": CERTIFICATE_ADVISORY, "proof_sha256": "e" * 64}]
        row["nested_certificate"] = {
            "pypi_attestation_sha256": "c" * 64, "archive_sha256": digest_file(archive),
            "image_config": image["config_digest"], "image_manifest": image["manifest"],
            "finding_id": target["finding_id"],
            "der_sha256s": sorted(digest_bytes(der) for der in (self.ders if ders is None else ders)),
        }
        return archive, manifest, raw

    def gate_scan(self, archive, manifest, name, *, block=False):
        with patch.object(sys, "path", [str(SCRIPTS), *sys.path]):
            import image_secret_gate as gate

            pinned = copy.deepcopy(manifest)
            pinned["scanner_manifest_sha256"] = gate.MANIFEST_SHA256
            raw = json.dumps(pinned).encode()
            output = self.root / name
            with patch.object(gate, "load_package_evidence", return_value=(raw, digest_bytes(raw), packages.REVIEW)):
                if block:
                    with self.assertRaises(gate.ScanRefusal) as refusal:
                        gate.verified_staging_scan(archive, output, component=COMPONENT)
                    self.assertEqual(refusal.exception.outcome, "scanner_findings")
                else:
                    gate.verified_staging_scan(archive, output, component=COMPONENT)
            return json.loads((output / "REPORT.json").read_bytes())

    def test_gate_and_warm_detector_cache_keep_the_original_finding(self):
        archive, manifest, raw = self.fixture()
        final = self.gate_scan(archive, manifest, "publication")
        self.assertEqual(raw["findings"], final["findings"])
        [decision] = final["package_evidence"]["entries"]
        self.assertEqual(decision["classification"], CERTIFICATE_ADVISORY)
        self.assertEqual(decision["nested_certificate"]["member_sha256"], digest_bytes(self.bundle))
        self.assertEqual(decision["nested_certificate"]["der_sha256s"],
                         manifest["entries"][0]["nested_certificate"]["der_sha256s"])
        cache = self.root / "certificates-cache.json"
        opts = ["--detector-cache-context", "d" * 64, "--detector-cache-output", str(cache)]
        cold = self.detector_scan(archive, COMPONENT, opts)
        warm = self.detector_scan(archive, COMPONENT, [*opts, "--detector-cache-input", str(cache)])
        self.assertEqual(warm["detector_cache"]["hits"], 1)
        for report in (cold, warm):
            section, assessed = self.classify(archive, report, self.policy(manifest), COMPONENT)
            self.assertEqual(section["entries"], [decision])
            self.assertEqual(assessed["counts"], final["counts"])

    def test_gate_refuses_changed_bindings(self):
        archive, manifest, _ = self.fixture()
        for key, value in (("entry", 1), ("file_sha256", "0" * 64), ("der_sha256s", "0" * 64),
                           ("image_config", "sha256:" + "0" * 64)):
            with self.subTest(key=key):
                changed = copy.deepcopy(manifest)
                row = changed["entries"][0]
                if key == "file_sha256":
                    row["file_sha256"] = row["package_member"]["sha256"] = value
                elif key == "entry":
                    row[key] = value
                elif key == "der_sha256s":
                    row["nested_certificate"][key][0] = value
                else:
                    row["nested_certificate"][key] = value
                report = self.gate_scan(archive, changed, key, block=True)
                self.assertEqual(report["package_evidence"]["entries"], [])

    def test_gate_refuses_incomplete_or_nonregular_bundles(self):
        for name, bundle, options in (
            ("extra", self.bundle + pem(self.ders[0]), {}),
            ("malformed", self.bundle + b"-----BEGIN CERTIFICATE-----\n", {}),
            ("inline-comment", self.bundle.rstrip() + b" # outside whole line\n", {}),
            ("ambiguous", self.bundle, {"mode": 0}),
            ("duplicate", self.bundle, {"duplicate": True}),
        ):
            with self.subTest(name=name):
                archive, manifest, _ = self.fixture(bundle=bundle, wheel_options=options)
                report = self.gate_scan(archive, manifest, name, block=True)
                self.assertEqual(report["package_evidence"]["entries"], [])
                self.assertIn(CERTIFICATE_RULE, {item["rule"] for item in report["blocking_findings"]})

    def test_gate_scans_comments_decoded_der_and_unrelated_members(self):
        url = b"https://fixture-user:fixture-password@example.invalid/resource"
        for name, bundle, ders, options, rule, approvals in (
            ("credential", b"# " + url + b"\n" + self.bundle, None, {}, "content.credential-url", 0),
            ("decoded", self.bundle.replace(pem(self.ders[-1]), pem(self.private_der)),
             [*self.ders[:-1], self.private_der], {}, "content.private-key-header", 0),
            ("sibling", self.bundle, None, {"sibling": url}, "content.credential-url", 1),
        ):
            with self.subTest(name=name):
                archive, manifest, _ = self.fixture(bundle=bundle, ders=ders, wheel_options=options)
                report = self.gate_scan(archive, manifest, name, block=True)
                self.assertEqual(len(report["package_evidence"]["entries"]), approvals)
                self.assertIn(rule, {item["rule"] for item in report["blocking_findings"]})
                if name == "decoded":
                    self.assertTrue(any(item["rule"] == rule and item.get("view") == "x509-der"
                                        for item in report["blocking_findings"]))

    def portable(self, *, bundle=None, ders=None, database=True):
        archive, manifest, _ = self.fixture(bundle=bundle, ders=ders)
        manifest["schema"] = OMARCHY_PORTABLE_SCHEMA
        manifest["packages"][0]["version"] = "3.14.8-1"
        [row] = manifest["entries"]
        for field in ("layer", "diff_id", "entry"):
            del row[field]
        for field in ("archive_sha256", "image_config", "image_manifest", "finding_id"):
            del row["nested_certificate"][field]
        wheel = wheel_bytes(self.bundle if bundle is None else bundle)
        record = f"#mtree\n./{WHEEL} type=file sha256digest={digest_bytes(wheel)}\n".encode()
        row.update(occurrences=1, ownership={"mtree_sha256": digest_bytes(record)})
        directory = f"{ROOT}/python-3.14.8-1"
        owner = [(VERSION, b"9\n"), (f"{directory}/desc", b"%NAME%\npython\n\n%VERSION%\n3.14.8-1\n\n%ARCH%\nx86_64\n"),
                 (f"{directory}/files", f"%FILES%\n{WHEEL}\n".encode()), (f"{directory}/mtree", record)]
        layer = fixtures.tar_bytes([(WHEEL, wheel), ("etc/machine-id", b""),
                                    ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id")),
                                    *(owner if database else ())])
        fixtures.oci_archive(archive, [layer], compress=False)
        return archive, manifest

    def test_portable_member_keeps_the_exact_inventory_and_arch_ownership(self):
        archive, manifest = self.portable()
        [decision] = self.gate_scan(archive, manifest, "portable")["package_evidence"]["entries"]
        self.assertEqual((decision["classification"], decision["member"]), (CERTIFICATE_ADVISORY, "0"))
        self.assertEqual(decision["nested_certificate"]["der_sha256s"],
                         manifest["entries"][0]["nested_certificate"]["der_sha256s"])
        self.assertEqual(set(decision["current_owner"]["pacman"]), {"desc", "files", "mtree"})
        url = b"https://fixture-user:fixture-password@example.invalid/resource"
        for name, options in (("unowned", {"database": False}), ("comment", {"bundle": b"# " + url + b"\n" + self.bundle})):
            with self.subTest(name=name):
                archive, manifest = self.portable(**options)
                report = self.gate_scan(archive, manifest, name, block=True)
                self.assertEqual(report["package_evidence"]["entries"], [])
                self.assertIn(CERTIFICATE_RULE, {item["rule"] for item in report["blocking_findings"]})
        for count in (nested_certificates.COUNT - 1, nested_certificates.COUNT + 1):
            with self.subTest(count=count):
                changed = copy.deepcopy(manifest)
                hashes = sorted(digest_bytes(der) for der in [*self.ders, self.private_der])[:count]
                changed["entries"][0]["nested_certificate"]["der_sha256s"] = hashes
                with self.assertRaises(ScanError):
                    self.policy(changed)
