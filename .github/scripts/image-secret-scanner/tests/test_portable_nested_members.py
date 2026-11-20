"""Prove reviewed members of a package-owned container through the portable Arch catalog."""

import copy
import io
import subprocess
import tarfile
import unittest
import zipfile
from unittest.mock import patch

from image_secrets import digest_frames, reviewed_literals
from image_secrets.arch_package_ownership import ROOT, VERSION
from image_secrets.common import ScanError, digest_bytes
from image_secrets.gate import (
    DIGEST_FRAME_ADVISORY,
    DIGEST_FRAME_RULE,
    FORMAT_MARKER_ADVISORY,
    URL_EXAMPLE_ADVISORY,
    URL_EXAMPLE_RULE,
)
from image_secrets.package_evidence import OMARCHY_PORTABLE_SCHEMA, PORTABLE_SCHEMA
from image_secrets.package_occurrences import prove_package_member
from tests import fixtures
from tests import test_derived_payload as derived_tests
from tests import test_package_policy as packages
from tests import test_reviewed_literals as literals

CONTAINER = "usr/share/fixture/classes/viewer.jar"
COMPONENT = "runtime-omarchy"
NAME, VERSION_ID = "fixture-viewer", "1.0-1"
IDENTITY = [("etc/machine-id", b""), ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id"))]
PUBLIC = ("fixture/Auth.py", packages.PUBLIC_SOURCE)
URL = ("fixture/Url.txt", literals.URL)
MARKER = ("fixture/Marker.txt", literals.MARKER)


def container(members):
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name, data in members:
            member = zipfile.ZipInfo(name, (2026, 1, 2, 3, 4, 6))
            member.create_system = 3
            member.external_attr = 0o100644 << 16
            member.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(member, data)
    return output.getvalue()


def mtree(payload, path=CONTAINER):
    return f"#mtree\n./{path} type=file sha256digest={digest_bytes(payload)}\n".encode()


def installed(record, *, version=VERSION_ID, path=CONTAINER):
    directory = f"{ROOT}/{NAME}-{version}"
    desc = f"%NAME%\n{NAME}\n\n%VERSION%\n{version}\n\n%ARCH%\nx86_64\n"
    return [(VERSION, b"9\n"), (f"{directory}/desc", desc.encode()),
            (f"{directory}/files", f"%FILES%\n{path}\n".encode()), (f"{directory}/mtree", record)]


class PortableNestedMemberTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    detector_scan = packages.PackageTests.detector_scan
    policy = staticmethod(packages.PackageTests.policy)
    publish = derived_tests.DerivedPayloadTests.publish

    def image(self, name, layers):
        archive = self.root / f"{name}.tar"
        fixtures.oci_archive(archive, layers)
        return archive

    def catalog(self, members, payload=None):
        payload = container(members) if payload is None else payload
        vendor = self.root / "vendor-package"
        vendor.write_bytes(fixtures.tar_bytes([(CONTAINER, payload)]))
        artifact = digest_bytes(vendor.read_bytes())
        base = {"component": COMPONENT, "platform": "linux/amd64", "path": CONTAINER,
                "file_sha256": digest_bytes(payload), "package_sha256": artifact,
                "package_member": prove_package_member(vendor, artifact, "0", self.root), "occurrences": 1,
                "ownership": {"mtree_sha256": digest_bytes(mtree(payload))}}
        rows = []
        for index, (name, data) in enumerate(members):
            row = dict(copy.deepcopy(base), member={"locator": str(index), "path": [name], "sha256": digest_bytes(data)})
            if (name, data) == PUBLIC:
                row["matches"] = [{"rule": packages.RULE, "candidate_sha256": digest_bytes(packages.SYMBOL), "line": 1,
                                   "explanation": "caller-supplied-value-expression", "proof_sha256": "4" * 64}]
            elif (name, data) in (URL, MARKER):
                rule = URL_EXAMPLE_RULE if (name, data) == URL else literals.MARKER_RULE
                spans = [{"offset": start, "size": end - start, "sha256": digest_bytes(data[start:end]),
                          "context": {"offset": 0, "size": len(data), "sha256": digest_bytes(data)},
                          "source": {"path": "vendor/" + name, "line": 1, "sha256": digest_bytes(data)}}
                         for start, end, _ in reviewed_literals.occurrences(data, rule)]
                row["reviewed_literals"] = {"findings": [{"rule": rule, "spans": spans}]}
                row["matches"] = [{"rule": rule, "candidate_sha256": None, "line": None, "proof_sha256": "4" * 64,
                                   "explanation": reviewed_literals.classification(rule)}]
            else:
                continue
            rows.append(row)
        return {"schema": OMARCHY_PORTABLE_SCHEMA, "scanner_manifest_sha256": packages.SCANNER,
                "packages": [{"name": NAME, "version": VERSION_ID, "architecture": "x86_64", "artifact_sha256": artifact,
                              "authority_type": "arch-signature", "authority_sha256": artifact}],
                "entries": rows}, payload

    def test_reviewed_members_resolve_in_every_rebuilt_layout(self):
        manifest, payload = self.catalog([PUBLIC, URL, MARKER])
        layer = fixtures.tar_bytes([*IDENTITY, *installed(mtree(payload)), (CONTAINER, payload)])
        raw = self.detector_scan(self.image("raw", [layer]), COMPONENT)
        self.assertEqual({(row["member"], row["rule"]) for row in raw["blocking_findings"]},
                         {("0", packages.RULE), ("1", URL_EXAMPLE_RULE), ("2", literals.MARKER_RULE)})
        reference = self.publish(self.image("reference", [layer]), manifest, "reference")
        rebuilt = self.publish(self.image("rebuilt", [fixtures.tar_bytes([("padding", b"ordinary")]), layer]),
                               manifest, "rebuilt")
        self.assertNotEqual(reference["archive_sha256"], rebuilt["archive_sha256"])
        rows = rebuilt["package_evidence"]["entries"]
        self.assertEqual(rebuilt["counts"]["blocking"], 0)
        self.assertEqual({(row["member"], row["classification"]) for row in rows},
                         {("0", "public-material"), ("1", URL_EXAMPLE_ADVISORY), ("2", FORMAT_MARKER_ADVISORY)})
        members = {str(index): digest_bytes(data) for index, (_, data) in enumerate([PUBLIC, URL, MARKER])}
        for row in rows:
            self.assertEqual((row["layer"], row["outer_file_sha256"]), (1, digest_bytes(payload)))
            self.assertEqual(row["file_sha256"], members[row["member"]])
            self.assertEqual(row["current_owner"]["pacman"]["mtree"]["sha256"], digest_bytes(mtree(payload)))

    def test_changed_container_path_version_or_ownership_keeps_every_member_blocking(self):
        manifest, payload = self.catalog([PUBLIC, URL, MARKER])
        changed = container([(PUBLIC[0], PUBLIC[1] + b"// locally modified\n"), URL, MARKER])
        moved = "usr/share/fixture/classes/copied.jar"
        good = installed(mtree(payload))
        for name, layers in (
            ("changed-bytes", [[*IDENTITY, *installed(mtree(changed)), (CONTAINER, changed)]]),
            ("wrong-path", [[*IDENTITY, *installed(mtree(payload, moved), path=moved), (moved, payload)]]),
            ("changed-version", [[*IDENTITY, *installed(mtree(payload), version="1.0-2"), (CONTAINER, payload)]]),
            ("missing-ownership", [[*IDENTITY, (CONTAINER, payload)]]),
            ("changed-mtree", [[*IDENTITY, *good[:-1], (good[-1][0], mtree(payload) + b"\n"), (CONTAINER, payload)]]),
            ("unexplained-copy", [[*IDENTITY, *good, (CONTAINER, payload)], [(CONTAINER, payload)]]),
        ):
            with self.subTest(name=name):
                archive = self.image(name, [fixtures.tar_bytes(layer) for layer in layers])
                refused = self.publish(archive, manifest, name, blocks=True)
                self.assertEqual(refused["package_evidence"]["entries"], [])
                self.assertGreaterEqual(refused["counts"]["blocking"], 3)

    def test_other_findings_in_the_member_or_container_keep_blocking(self):
        key = subprocess.run(["openssl", "genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:prime256v1",
                              "-outform", "DER"], capture_output=True, check=True, timeout=10).stdout
        sibling = ("fixture/material.bin", fixtures.fake_key())
        for name, members, approved in (
            ("sibling-key", [PUBLIC, URL, MARKER, sibling], {"0", "1", "2"}),
            ("marker-with-key", [PUBLIC, URL, (MARKER[0], MARKER[1] + key)], {"0", "1"}),
        ):
            with self.subTest(name=name):
                manifest, payload = self.catalog(members)
                archive = self.image(name, [fixtures.tar_bytes([*IDENTITY, *installed(mtree(payload)),
                                                                (CONTAINER, payload)])])
                refused = self.publish(archive, manifest, name, blocks=True)
                self.assertEqual({row["member"] for row in refused["package_evidence"]["entries"]}, approved)
                blocked = {(row["member"], row["rule"]) for row in refused["blocking_findings"]}
                if name == "sibling-key":
                    self.assertIn(("3", literals.MARKER_RULE), blocked)
                else:
                    self.assertIn(("2", literals.MARKER_RULE), blocked)
                    self.assertIn(("2", "content.binary-private-key"), blocked)

    def test_member_rows_load_only_with_supported_container_proofs(self):
        manifest, _ = self.catalog([PUBLIC, URL, MARKER])
        self.policy(manifest)
        top = dict(copy.deepcopy(manifest["entries"][0]), member=None)
        self.policy(dict(manifest, entries=[*manifest["entries"], top]))

        def altered(change):
            value = copy.deepcopy(manifest)
            change(value["entries"])
            return value

        for name, change in (
            ("unsupported-member-proof", lambda rows: rows[0].update(digest_frames={})),
            ("derived-member", lambda rows: rows[0].update(derived_payload={})),
            ("disagreeing-ownership", lambda rows: rows[1].update(ownership={"mtree_sha256": "5" * 64})),
            ("duplicate-locator", lambda rows: rows.append(copy.deepcopy(rows[0]))),
            ("top-level-certificate", lambda rows: rows.append(dict(top, nested_certificate={}))),
        ):
            with self.subTest(name=name), self.assertRaises(ScanError):
                self.policy(altered(change))
        # The nested reader rechecks only literal proofs for the container itself.
        frames = dict(top, digest_frames={}, matches=[{
            "rule": DIGEST_FRAME_RULE, "candidate_sha256": None, "line": None,
            "explanation": DIGEST_FRAME_ADVISORY, "proof_sha256": "4" * 64}])
        with patch.object(digest_frames, "validate"), patch.object(digest_frames, "acceptable_match"):
            self.policy(dict(manifest, entries=[frames]))
            with self.assertRaises(ScanError):
                self.policy(dict(manifest, entries=[*manifest["entries"], frames]))
        # A CMA producer copy is proved by reserved inputs, never as a package-owned container.
        producer = copy.deepcopy(manifest)
        producer["packages"][0]["authority_type"] = "pinned-upstream"
        for row in producer["entries"]:
            row["ownership"] = {"build_inputs": {"api/crates/layer4-runtime/Dockerfile": "6" * 64}}
        self.policy(dict(producer, entries=[dict(producer["entries"][0], member=None)]))
        with self.assertRaises(ScanError):
            self.policy(dict(producer, entries=producer["entries"][:1]))
        # The default runtime catalog has no nested-member acceptance path.
        runtime = copy.deepcopy(manifest)
        runtime["schema"] = PORTABLE_SCHEMA
        runtime["packages"][0]["authority_type"] = "debian-index"
        [row] = runtime["entries"][:1]
        row.update(component="runtime", ownership={})
        self.policy(dict(runtime, entries=[dict(row, member=None)]))
        with self.assertRaises(ScanError):
            self.policy(dict(runtime, entries=[row]))


if __name__ == "__main__":
    unittest.main()
