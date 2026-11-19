"""Exercise exact Arch CA payloads and their final-image alias through the gate."""

import copy
import tarfile
import unittest
from pathlib import Path

from image_secrets import arch_trust
from image_secrets.common import digest_bytes
from image_secrets.gate import ARCH_CERTIFICATE_ADVISORY, CERTIFICATE_RULE
from tests import fixtures
from tests import test_nested_certificates as certificates
from tests import test_package_policy as packages

COMPONENT = arch_trust.COMPONENT
SCRIPTS = Path(__file__).resolve().parents[2]
EMPTY = "etc/ca-certificates/extracted/objsign-ca-bundle.pem"


class ArchTrustTests(unittest.TestCase):
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

    def fixture(self, *, subsequent=(), link=arch_trust.LINK, extra=(), email_prefix=b"", gnupg_der=None):
        gnupg_der = self.ders[0] if gnupg_der is None else gnupg_der
        inventories = {arch_trust.EMAIL: self.ders[:88], arch_trust.TLS: self.ders, arch_trust.GNUPG: [gnupg_der]}
        files = {path: (b"# Generated synthetic trust store\n\n" if path != arch_trust.GNUPG else b"")
                 + b"".join(certificates.pem(der) for der in ders) for path, ders in inventories.items()}
        files[arch_trust.EMAIL] = email_prefix + files[arch_trust.EMAIL]
        names = list(files)
        base = fixtures.tar_bytes([*files.items(), (arch_trust.ALIAS, (tarfile.SYMTYPE, link)),
                                   ("etc/machine-id", b""),
                                   ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id")), *extra])
        layers = [base, *(fixtures.tar_bytes(entries) for entries in subsequent)]
        archive = self.root / "arch-trust.tar"
        fixtures.docker_archive(archive, layers)
        raw = self.detector_scan(archive, COMPONENT)
        findings = {row["entry"]: row for row in raw["findings"]
                    if row["rule"] == CERTIFICATE_RULE and row["layer"] == 0}
        package_sha = digest_bytes(fixtures.tar_bytes([(arch_trust.GNUPG, files[arch_trust.GNUPG])]))
        package = {"name": "gnupg", "version": "2.4.9-3", "architecture": "x86_64",
                   "artifact_sha256": package_sha, "authority_type": "arch-signature", "authority_sha256": "c" * 64}
        gnupg_member = {"locator": "0", "path": [arch_trust.GNUPG], "type": "regular",
                        "sha256": digest_bytes(files[arch_trust.GNUPG])}
        records = [{"path": path, "entry": index, "finding_id": findings[index]["finding_id"],
                    "file_sha256": digest_bytes(files[path]), "der_sha256s": sorted(map(digest_bytes, inventories[path])),
                    "proof_sha256": "d" * 64, "native": None}
                   for index, path in enumerate(names)]
        records[names.index(arch_trust.GNUPG)]["native"] = {
            "package_sha256": package_sha, "member": gnupg_member, "mtree_sha256": "e" * 64,
        }
        manifest = {
            "schema": "cma-staging-package-evidence-v1", "scanner_manifest_sha256": packages.SCANNER,
            "packages": [package], "entries": [],
            "arch_trust": {
                "base": {"index_digest": "sha256:" + "a" * 64, "platform_manifest_digest": "sha256:" + "b" * 64,
                         "layer_diff_id": "sha256:" + digest_bytes(base)},
                "files": records,
                "alias": {"path": arch_trust.ALIAS, "entry": len(names),
                          "finding_id": findings[len(names)]["finding_id"],
                          "link_target": arch_trust.LINK, "proof_sha256": "f" * 64},
            },
        }
        return archive, manifest, raw

    def test_publication_and_warm_cache_preserve_all_four_raw_findings(self):
        archive, manifest, raw = self.fixture()
        final = self.gate_scan(archive, manifest, "publication")
        self.assertEqual(raw["findings"], final["findings"])
        results = {row["arch_trust"]["public_path"]: row for row in final["package_evidence"]["entries"]}
        self.assertEqual(set(results), {*arch_trust.FILES, arch_trust.ALIAS})
        self.assertTrue(all(row["classification"] == ARCH_CERTIFICATE_ADVISORY for row in results.values()))
        self.assertEqual(results[arch_trust.ALIAS]["arch_trust"]["target_finding_id"],
                         results[arch_trust.TLS]["finding_id"])
        self.assertEqual(results[arch_trust.GNUPG]["arch_trust"]["native"]["package"], manifest["packages"][0])
        cache = self.root / "arch-cache.json"
        options = ["--detector-cache-context", "d" * 64, "--detector-cache-output", str(cache)]
        self.detector_scan(archive, COMPONENT, options)
        warm = self.detector_scan(archive, COMPONENT, [*options, "--detector-cache-input", str(cache)])
        self.assertEqual(warm["detector_cache"]["hits"], 1)
        section, assessed = self.classify(archive, warm, self.policy(manifest), COMPONENT)
        self.assertEqual(section["entries"], final["package_evidence"]["entries"])
        self.assertEqual(assessed["counts"], final["counts"])
        unrelated = self.detector_scan(archive, "runtime")
        self.assertEqual(self.classify(archive, unrelated, self.policy(manifest))[0]["entries"], [])

    def test_all_layers_must_leave_the_alias_and_target_unambiguous(self):
        parent = "etc/ca-certificates/extracted"
        for name, link, later in (
            ("changed-link", "../ca-certificates/extracted/email-ca-bundle.pem", ()),
            ("hidden-target", arch_trust.LINK, ([(parent + "/.wh.tls-ca-bundle.pem", b"")],)),
            ("overlaid-target", arch_trust.LINK, ([(arch_trust.TLS, self.bundle)],)),
            ("overlaid-ancestor", arch_trust.LINK, ([(parent, (tarfile.SYMTYPE, "/tmp"))],)),
        ):
            with self.subTest(name=name):
                archive, manifest, _ = self.fixture(link=link, subsequent=later)
                report = self.gate_scan(archive, manifest, name, block=True)
                paths = {row["arch_trust"]["public_path"] for row in report["package_evidence"]["entries"]}
                self.assertEqual(paths, set(arch_trust.FILES))
                self.assertIn(manifest["arch_trust"]["alias"]["finding_id"],
                              {row["finding_id"] for row in report["blocking_findings"]})

    def test_wrong_der_inventory_denies_the_target_and_its_alias(self):
        archive, manifest, raw = self.fixture()
        for key in ("der_sha256s", "file_sha256", "entry"):
            with self.subTest(key=key):
                wrong = copy.deepcopy(manifest)
                tls = next(row for row in wrong["arch_trust"]["files"] if row["path"] == arch_trust.TLS)
                if key == "der_sha256s":
                    tls[key][0] = "0" * 64
                elif key == "file_sha256":
                    tls[key] = "0" * 64
                else:
                    tls[key] = 900
                section, assessed = self.classify(archive, raw, self.policy(wrong), COMPONENT)
                self.assertEqual({row["arch_trust"]["public_path"] for row in section["entries"]},
                                 {arch_trust.EMAIL, arch_trust.GNUPG})
                self.assertIn(tls["finding_id"], {row["finding_id"] for row in assessed["blocking_findings"]})

    def test_comments_der_and_empty_files_keep_their_blockers(self):
        comment = b"# https://fixture-user:fixture-password@example.invalid/\n"
        archive, manifest, _ = self.fixture(email_prefix=comment, gnupg_der=self.private_der, extra=[(EMPTY, b"")])
        report = self.gate_scan(archive, manifest, "unresolved", block=True)
        self.assertEqual({row["arch_trust"]["public_path"] for row in report["package_evidence"]["entries"]},
                         {arch_trust.TLS, arch_trust.ALIAS})
        blockers = report["blocking_findings"]
        self.assertTrue(any(row["rule"] == "content.credential-url" for row in blockers))
        self.assertTrue(any(row["rule"] == "content.private-key-header" and row.get("view") == "x509-der"
                            for row in blockers))
        self.assertTrue(any(row["rule"] == CERTIFICATE_RULE and row["entry"] == 6 for row in blockers))
