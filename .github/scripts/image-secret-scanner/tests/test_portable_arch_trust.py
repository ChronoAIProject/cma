"""Exercise the rebuild-stable Omarchy trust-store catalog through detection, receipt and gate."""

import copy
import json
import sys
import tarfile
import unittest
from pathlib import Path
from unittest.mock import patch

from image_secrets import arch_trust
from image_secrets.arch_package_ownership import ROOT, VERSION
from image_secrets.common import ScanError, digest_bytes
from image_secrets.gate import ARCH_CERTIFICATE_ADVISORY, CERTIFICATE_RULE
from image_secrets.package_evidence import OMARCHY_PORTABLE_SCHEMA, PORTABLE_SCHEMA
from tests import fixtures
from tests import test_nested_certificates as certificates
from tests import test_package_policy as packages

COMPONENT = arch_trust.COMPONENT
SCRIPTS = Path(__file__).resolve().parents[2]
EMAIL, TLS, GNUPG, ALIAS = arch_trust.EMAIL, arch_trust.TLS, arch_trust.GNUPG, arch_trust.ALIAS
EXTRACTED = "etc/ca-certificates/extracted"
IDENTITY = [("etc/machine-id", b""), ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id"))]
REVIEWED = {*arch_trust.FILES, ALIAS}


def owner(mtree, *, name="gnupg", version="2.4.9-3", listed=GNUPG, alpm=True):
    directory = f"{ROOT}/{name}-{version}"
    desc = f"%NAME%\n{name}\n\n%VERSION%\n{version}\n\n%ARCH%\nx86_64\n".encode()
    return [*([(VERSION, b"9\n")] if alpm else []), (f"{directory}/desc", desc),
            (f"{directory}/files", f"%FILES%\n{listed}\n".encode()), (f"{directory}/mtree", mtree)]


class PortableArchTrustTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    detector_scan = packages.PackageTests.detector_scan
    classify = packages.PackageTests.classify
    policy = staticmethod(packages.PackageTests.policy)
    gate_scan = certificates.NestedCertificateTests.gate_scan

    @classmethod
    def setUpClass(cls):
        certificates.NestedCertificateTests.setUpClass.__func__(cls)

    def fixture(self, name, *, oci=False, lead=(), later=(), link=arch_trust.LINK, email_prefix=b"",
                gnupg_der=None, tls_ders=None, database=None, extra=()):
        inventories = {EMAIL: self.ders[:88], TLS: self.ders if tls_ders is None else tls_ders,
                       GNUPG: [self.ders[0] if gnupg_der is None else gnupg_der]}
        files = {path: (b"" if path == GNUPG else b"# Generated synthetic trust store\n\n")
                 + b"".join(certificates.pem(der) for der in ders) for path, ders in inventories.items()}
        files[EMAIL] = email_prefix + files[EMAIL]
        mtree = f"#mtree\n./{GNUPG} type=file sha256digest={digest_bytes(files[GNUPG])}\n".encode()
        installed = owner(mtree) if database is None else database(mtree)
        base = fixtures.tar_bytes([*lead, *files.items(), (ALIAS, (tarfile.SYMTYPE, link)), *installed,
                                   *IDENTITY, *extra])
        archive = self.root / f"{name}.tar"
        (fixtures.oci_archive if oci else fixtures.docker_archive)(
            archive, [base, *(fixtures.tar_bytes(entries) for entries in later)])
        package_sha = digest_bytes(fixtures.tar_bytes([(GNUPG, files[GNUPG])]))
        records = [{"path": path, "file_sha256": digest_bytes(files[path]), "certificates": arch_trust.FILES[path],
                    "der_sha256s": sorted(map(digest_bytes, ders))[:arch_trust.FILES[path]],
                    "proof_sha256": "d" * 64, "native": None} for path, ders in inventories.items()]
        records[-1]["native"] = {"package_sha256": package_sha, "mtree_sha256": digest_bytes(mtree),
                                 "member": {"locator": "0", "path": [GNUPG], "type": "regular",
                                            "sha256": digest_bytes(files[GNUPG])}}
        manifest = {
            "schema": OMARCHY_PORTABLE_SCHEMA, "scanner_manifest_sha256": packages.SCANNER, "entries": [],
            "packages": [{"name": "gnupg", "version": "2.4.9-3", "architecture": "x86_64",
                          "artifact_sha256": package_sha, "authority_type": "arch-signature",
                          "authority_sha256": "c" * 64}],
            "trust_store": {"files": records, "alias": {"path": ALIAS, "link_target": arch_trust.LINK,
                                                        "proof_sha256": "f" * 64}},
        }
        return archive, manifest

    def decided(self, archive, manifest, raw=None):
        raw = self.detector_scan(archive, COMPONENT) if raw is None else raw
        section, assessed = self.classify(archive, raw, self.policy(manifest), COMPONENT)
        paths = {row["arch_trust"]["public_path"] for row in section["entries"]}
        blocked = {row["path"] for row in assessed["blocking_findings"]}
        return paths, blocked, assessed

    @staticmethod
    def digest(path):
        return "path-sha256:" + digest_bytes(path.encode())

    def test_one_catalog_resolves_rebuilt_layouts_with_current_identities(self):
        archive, manifest = self.fixture("reference")
        reference = self.gate_scan(archive, manifest, "reference")
        rebuilt_archive, _ = self.fixture("rebuilt", oci=True, lead=[("usr/share/doc/notes.txt", b"ordinary")],
                                          later=[[("opt/added.txt", b"innocuous")]])
        rebuilt = self.gate_scan(rebuilt_archive, manifest, "rebuilt")
        for report in (reference, rebuilt):
            self.assertEqual(report["blocking_findings"], [])
            rows = {row["arch_trust"]["public_path"]: row for row in report["package_evidence"]["entries"]}
            self.assertEqual(set(rows), REVIEWED)
            self.assertTrue(all(row["classification"] == ARCH_CERTIFICATE_ADVISORY
                                and row["arch_trust"]["catalog"] == "portable"
                                and row["arch_trust"]["namespace_verified"] is True for row in rows.values()))
            self.assertEqual(rows[ALIAS]["arch_trust"]["target_finding_id"], rows[TLS]["finding_id"])
            native = rows[GNUPG]["arch_trust"]["native"]
            self.assertEqual(native["package"], manifest["packages"][0])
            self.assertEqual({record["layer"] for record in native["current_owner"]["pacman"].values()}, {0})
        before = {row["arch_trust"]["public_path"]: row for row in reference["package_evidence"]["entries"]}
        after = {row["arch_trust"]["public_path"]: row for row in rebuilt["package_evidence"]["entries"]}
        for path in REVIEWED:
            for field in ("entry", "finding_id", "layer_digest", "image_config"):
                self.assertNotEqual(before[path][field], after[path][field])
            self.assertEqual(before[path]["file_sha256"], after[path]["file_sha256"])
        cache = self.root / "portable-cache.json"
        options = ["--detector-cache-context", "d" * 64, "--detector-cache-output", str(cache)]
        self.detector_scan(archive, COMPONENT, options)
        warm = self.detector_scan(archive, COMPONENT, [*options, "--detector-cache-input", str(cache)])
        self.assertEqual(warm["detector_cache"]["hits"], 1)
        section, assessed = self.classify(archive, warm, self.policy(manifest), COMPONENT)
        self.assertEqual(section["entries"], reference["package_evidence"]["entries"])
        self.assertEqual(assessed["counts"], reference["counts"])

    def test_catalog_bytes_inventory_and_package_identity_must_match(self):
        archive, manifest = self.fixture("catalog")
        raw = self.detector_scan(archive, COMPONENT)

        def record(catalog, path):
            return next(row for row in catalog["trust_store"]["files"] if row["path"] == path)

        for name, change, denied in (
            ("tls-file", lambda c: record(c, TLS).update(file_sha256="0" * 64), {TLS, ALIAS}),
            ("tls-der", lambda c: record(c, TLS)["der_sha256s"].__setitem__(0, "0" * 64), {TLS, ALIAS}),
            ("email-file", lambda c: record(c, EMAIL).update(file_sha256="0" * 64), {EMAIL}),
            ("gnupg-mtree", lambda c: record(c, GNUPG)["native"].update(mtree_sha256="0" * 64), {GNUPG}),
            ("gnupg-version", lambda c: c["packages"][0].update(version="2.4.9-4"), {GNUPG}),
            ("no-alias", lambda c: c["trust_store"].update(alias=None), {ALIAS}),
        ):
            with self.subTest(name=name):
                catalog = copy.deepcopy(manifest)
                change(catalog)
                paths, blocked, _ = self.decided(archive, catalog, raw)
                self.assertEqual(paths, REVIEWED - denied)
                self.assertEqual(blocked, {self.digest(path) for path in denied})

    def content(self, path):
        if path == GNUPG:
            return certificates.pem(self.ders[0])
        ders = self.ders[:88] if path == EMAIL else self.ders
        return b"# Generated synthetic trust store\n\n" + b"".join(certificates.pem(der) for der in ders)

    def mtree(self):
        return f"#mtree\n./{GNUPG} type=file sha256digest={digest_bytes(self.content(GNUPG))}\n".encode()

    def test_later_layers_paths_and_bundle_contents_keep_their_blockers(self):
        _, reviewed = self.fixture("reviewed")
        copy_path = "opt/vendor/tls-ca-bundle.pem"
        for name, options, denied in (
            ("hidden-target", {"later": [[(EXTRACTED + "/.wh.tls-ca-bundle.pem", b"")]]}, {TLS, ALIAS}),
            ("hidden-gnupg", {"later": [[("usr/share/gnupg/.wh..wh..opq", b"")]]}, {GNUPG}),
            ("reinstalled-gnupg", {"later": [[(GNUPG, self.content(GNUPG))]]}, {GNUPG}),
            ("replaced-tls", {"later": [[(TLS, self.content(TLS))]]}, {TLS, ALIAS}),
            ("linked-ancestor", {"later": [[(EXTRACTED, (tarfile.SYMTYPE, "/tmp"))]]}, {EMAIL, TLS, ALIAS}),
            ("changed-link", {"link": "../ca-certificates/extracted/email-ca-bundle.pem"}, {ALIAS}),
            ("extra-certificate", {"tls_ders": [*self.ders, self.ders[0]]}, {TLS, ALIAS}),
            ("fewer-certificates", {"tls_ders": self.ders[1:]}, {TLS, ALIAS}),
            ("app-copy", {"later": [[(copy_path, self.content(TLS))]]}, set()),
        ):
            with self.subTest(name=name):
                archive, _ = self.fixture(name, **options)
                paths, blocked, _ = self.decided(archive, reviewed)
                self.assertEqual(paths, REVIEWED - denied)
                expected = {self.digest(path) for path in denied} or {self.digest(copy_path)}
                self.assertEqual(blocked, expected)

    def test_key_material_in_any_view_keeps_the_file_blocking(self):
        comment = b"# https://fixture-user:fixture-password@example.invalid/\n"
        archive, manifest = self.fixture("key-material", email_prefix=comment, gnupg_der=self.private_der)
        report = self.gate_scan(archive, manifest, "key-material", block=True)
        self.assertEqual({row["arch_trust"]["public_path"] for row in report["package_evidence"]["entries"]},
                         {TLS, ALIAS})
        blockers = report["blocking_findings"]
        self.assertTrue(any(row["rule"] == "content.credential-url" for row in blockers))
        self.assertTrue(any(row["rule"] == "content.private-key-header" and row.get("view") == "x509-der"
                            for row in blockers))
        self.assertEqual({row["path"] for row in blockers if row["rule"] == CERTIFICATE_RULE},
                         {self.digest(EMAIL), self.digest(GNUPG)})

    def test_gnupg_certificate_requires_its_current_pacman_owner(self):
        for name, database in (
            ("no-database", lambda mtree: []),
            ("other-version", lambda mtree: owner(mtree, version="2.4.9-4")),
            ("competing-owner", lambda mtree: [*owner(mtree), *owner(mtree, name="other", alpm=False)]),
            ("mtree-without-file", lambda mtree: owner(b"#mtree\n")),
            ("unlisted-file", lambda mtree: owner(mtree, listed="usr/share/gnupg/other.pem")),
        ):
            with self.subTest(name=name):
                archive, manifest = self.fixture(name, database=database)
                paths, blocked, _ = self.decided(archive, manifest)
                self.assertEqual(paths, REVIEWED - {GNUPG})
                self.assertEqual(blocked, {self.digest(GNUPG)})
        with self.subTest(name="database-in-later-layer"):
            archive, manifest = self.fixture("later-database", database=lambda _: [], later=[owner(self.mtree())])
            paths, blocked, _ = self.decided(archive, manifest)
            self.assertEqual(paths, REVIEWED - {GNUPG})
            self.assertEqual(blocked, {self.digest(GNUPG)})

    def test_portable_catalog_refuses_image_identity_and_unreviewed_shapes(self):
        _, manifest = self.fixture("shape")
        self.policy(manifest)

        def tls(catalog):
            return next(row for row in catalog["trust_store"]["files"] if row["path"] == TLS)

        for name, change in (
            ("entry", lambda c: tls(c).update(entry=3)),
            ("finding", lambda c: tls(c).update(finding_id="a" * 64)),
            ("base", lambda c: c["trust_store"].update(base={"layer_diff_id": "sha256:" + "a" * 64})),
            ("count", lambda c: tls(c).update(certificates=117)),
            ("unsorted", lambda c: tls(c)["der_sha256s"].reverse()),
            ("tls-native", lambda c: tls(c).update(native=c["trust_store"]["files"][-1]["native"])),
            ("gnupg-unowned", lambda c: c["trust_store"]["files"][-1].update(native=None)),
            ("wrong-member", lambda c: c["trust_store"]["files"][-1]["native"]["member"].update(sha256="0" * 64)),
            ("unknown-path", lambda c: tls(c).update(path="etc/ssl/certs/ca-certificates.crt")),
            ("link", lambda c: c["trust_store"]["alias"].update(link_target="tls-ca-bundle.pem")),
            ("duplicate", lambda c: c["trust_store"]["files"].append(copy.deepcopy(tls(c)))),
            ("default-runtime", lambda c: c.update(schema=PORTABLE_SCHEMA)),
            ("pinned-image", lambda c: c.update(schema="cma-staging-package-evidence-v1")),
            ("both-forms", lambda c: c.update(arch_trust=c["trust_store"])),
        ):
            with self.subTest(name=name):
                catalog = copy.deepcopy(manifest)
                change(catalog)
                with self.assertRaises(ScanError):
                    self.policy(catalog)

    def test_receipt_validation_refuses_altered_portable_rows(self):
        archive, manifest = self.fixture("receipt")
        report = self.gate_scan(archive, manifest, "receipt")
        rows = {row["finding_id"]: row for row in report["package_evidence"]["entries"]}
        by_path = {row["arch_trust"]["public_path"]: row for row in rows.values()}
        with patch.object(sys, "path", [str(SCRIPTS), *sys.path]):
            import image_secret_gate as gate

            for row in rows.values():
                gate.validate_arch_certificate(row, rows, report["archive_sha256"])
            for name, path, change in (
                ("namespace", ALIAS, lambda r: r["arch_trust"].update(namespace_verified=False)),
                ("catalog", TLS, lambda r: r["arch_trust"].pop("catalog")),
                ("target", ALIAS, lambda r: r["arch_trust"].update(target_finding_id=by_path[EMAIL]["finding_id"])),
                ("inventory", EMAIL, lambda r: r["arch_trust"]["der_sha256s"].pop()),
                ("owner-layer", GNUPG, lambda r: r["arch_trust"]["native"]["current_owner"]["pacman"]["desc"]
                 .update(layer=1)),
                ("owner-missing", GNUPG, lambda r: r["arch_trust"]["native"].pop("current_owner")),
                ("repack", TLS, lambda r: r["arch_trust"].update(repack={})),
            ):
                with self.subTest(name=name):
                    altered = copy.deepcopy(rows)
                    row = next(item for item in altered.values() if item["arch_trust"]["public_path"] == path)
                    change(row)
                    with self.assertRaises((gate.ScanRefusal, KeyError)):
                        gate.validate_arch_certificate(row, altered, report["archive_sha256"])


if __name__ == "__main__":
    unittest.main()
