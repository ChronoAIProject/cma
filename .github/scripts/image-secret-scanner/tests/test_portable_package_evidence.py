"""Keep reviewed default-runtime evidence stable across rebuilt layer layouts."""

import copy
import json
import sys
import tarfile
import unittest
from pathlib import Path
from unittest.mock import patch

from image_secrets.common import ScanError, digest_bytes
from image_secrets.package_evidence import PORTABLE_SCHEMA
from image_secrets.portable_ownership import STATUS_PATH
from tests import fixtures
from tests import test_derived_payload as derived
from tests import test_package_policy as packages

SCRIPTS = Path(__file__).resolve().parents[2]
MACHINE_IDENTITY = [("etc/machine-id", b""),
                    ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id"))]
DOCKERFILE = "api/crates/layer4-runtime/Dockerfile"


def dpkg_status(name, version, architecture):
    return (STATUS_PATH, f"Package: {name}\nVersion: {version}\nArchitecture: {architecture}\n"
                         "Status: install ok installed\n".encode())


def portable_manifest(manifest, *, source_inputs=None):
    catalog = copy.deepcopy(manifest)
    catalog["schema"] = PORTABLE_SCHEMA
    rows = {}
    for row in catalog["entries"]:
        for field in ("layer", "diff_id", "entry"):
            del row[field]
        key = (row["path"], row["file_sha256"])
        if "derived_payload" in row:
            origin = row["derived_payload"]
            for field in ("archive_sha256", "image_config", "image_manifest", "findings"):
                del origin[field]
        ownership = {"build_inputs": source_inputs} if source_inputs else {}
        if row.get("derived_payload", {}).get("kind") == "compiled-executable":
            ownership["debian_owner"] = {"name": "openssl", "version": "1.0-1", "architecture": "amd64"}
        if key in rows:
            rows[key]["occurrences"] += 1
        else:
            rows[key] = dict(row, occurrences=1, ownership=ownership)
    catalog["entries"] = list(rows.values())
    return catalog


class PortablePackageTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    manifest = packages.PackageTests.manifest
    policy = staticmethod(packages.PackageTests.policy)
    detector_scan = packages.PackageTests.detector_scan
    fixture = derived.DerivedPayloadTests.fixture
    compiled = derived.DerivedPayloadTests.compiled
    binding = derived.DerivedPayloadTests.binding
    publish = derived.DerivedPayloadTests.publish

    def test_same_catalog_resolves_repacked_entries_and_rejects_app_overlay(self):
        body = (packages.SOURCE, packages.PUBLIC_SOURCE)
        owner = dpkg_status("example-parser", "1.0", "all")
        vendor = fixtures.tar_bytes([body])
        first = self.root / "first.tar"
        fixtures.docker_archive(first, [fixtures.tar_bytes([body, owner, *MACHINE_IDENTITY])])
        reference = self.manifest(vendor, vendor)
        reference["packages"][0]["authority_type"] = "debian-index"
        policy = portable_manifest(reference)
        original = self.publish(first, policy, "original")
        second = self.root / "second.tar"
        next_layer = fixtures.tar_bytes([("added.txt", b"innocuous"), body, owner, *MACHINE_IDENTITY])
        fixtures.oci_archive(second, [fixtures.tar_bytes([("base", b"ordinary")]), next_layer])
        rebuilt = self.publish(second, policy, "rebuilt")
        first_evidence = original["package_evidence"]["entries"][0]
        second_evidence = rebuilt["package_evidence"]["entries"][0]
        for field in ("layer", "entry", "layer_digest", "image_config", "image_manifest", "finding_id"):
            self.assertNotEqual(first_evidence[field], second_evidence[field])
        self.assertEqual(second_evidence["outer_file_sha256"], first_evidence["outer_file_sha256"])
        self.assertEqual(rebuilt["package_evidence"]["archive_sha256"], digest_bytes(second.read_bytes()))
        overlay = self.root / "overlay.tar"
        fixtures.docker_archive(overlay, [next_layer, vendor])
        refused = self.publish(overlay, policy, "overlay-check", blocks=True)
        self.assertEqual(refused["counts"]["blocking"], 2)
        self.assertEqual(refused["package_evidence"]["entries"], [])
        changed = self.root / "different-owner.tar"
        fixtures.oci_archive(changed, [fixtures.tar_bytes([
            body, dpkg_status("example-parser", "1.1", "all"), *MACHINE_IDENTITY])])
        refused = self.publish(changed, policy, "changed-owner", blocks=True)
        self.assertEqual(refused["counts"]["blocking"], 1)
        self.assertEqual(refused["package_evidence"]["entries"], [])

    def test_two_rebuilt_derived_occurrences_receive_current_individual_receipts(self):
        archive, manifest, _ = self.compiled("compiled", copies=2)
        selected = self.root / "reserved-source"
        recipe = selected / DOCKERFILE
        recipe.parent.mkdir(parents=True)
        recipe.write_bytes(b"reviewed producer input")
        catalog = portable_manifest(manifest, source_inputs={DOCKERFILE: digest_bytes(recipe.read_bytes())})
        with tarfile.open(archive) as export:
            with tarfile.open(fileobj=export.extractfile("layer0/layer.tar")) as layer:
                compiled = layer.extractfile(derived.OPENSSL).read()
        owner = dpkg_status("openssl", "1.0-1", "amd64")
        first = fixtures.tar_bytes([(derived.OPENSSL, compiled), owner, *MACHINE_IDENTITY],
                                   modes={derived.OPENSSL: 0o755})
        second = fixtures.tar_bytes([("another", b"ordinary"), (derived.OPENSSL, compiled)],
                                    modes={derived.OPENSSL: 0o755})
        fixtures.docker_archive(archive, [first, second])
        installed = self.publish(archive, catalog, "compiled-first", source=selected)
        rebuilt = self.root / "derived-rebuild.tar"
        fixtures.oci_archive(rebuilt, [fixtures.tar_bytes([("padding", b"ordinary")]), first, second])
        receipt = self.publish(rebuilt, catalog, "compiled-rebuilt", source=selected)
        rows = receipt["package_evidence"]["entries"]
        self.assertEqual([row["layer"] for row in rows], [1, 2])
        self.assertEqual(len({row["finding_id"] for row in rows}), 2)
        self.assertNotEqual(installed["archive_sha256"], receipt["archive_sha256"])
        for row in rows:
            self.assertEqual(row["derived_payload"]["archive_sha256"], receipt["archive_sha256"])
            self.assertEqual(row["derived_payload"]["findings"], [row["finding_id"]])
            self.assertEqual(row["current_owner"]["dpkg_status"]["layer"], 1)
        recipe.write_bytes(b"changed producer input")
        denied = self.publish(rebuilt, catalog, "changed-source", blocks=True, source=selected)
        self.assertEqual(denied["counts"]["blocking"], 2)
        self.assertEqual(denied["package_evidence"]["entries"], [])
        excluded = copy.deepcopy(catalog)
        del excluded["entries"][0]["derived_payload"]["producer"]
        with self.assertRaises(ScanError):
            self.policy(excluded)

    def test_portable_catalog_refuses_old_identity_fields_and_other_components(self):
        vendor = fixtures.tar_bytes([(packages.SOURCE, packages.PUBLIC_SOURCE)])
        reference = self.manifest(vendor, vendor)
        reference["packages"][0]["authority_type"] = "debian-index"
        catalog = portable_manifest(reference)
        for changes in ({"entry": 0}, {"component": "runtime-omarchy"}, {"occurrences": 2}):
            with self.subTest(changes=changes):
                altered = copy.deepcopy(catalog)
                altered["entries"][0].update(changes)
                with self.assertRaises(ScanError):
                    self.policy(altered)
        with patch.object(sys, "path", [str(SCRIPTS), *sys.path]):
            import image_secret_gate as gate

            pinned = json.dumps(dict(catalog, scanner_manifest_sha256=gate.MANIFEST_SHA256)).encode()
            archive = self.root / "wrong-component.tar"
            fixtures.docker_archive(archive, [fixtures.tar_bytes([
                (packages.SOURCE, packages.PUBLIC_SOURCE), *MACHINE_IDENTITY])])
            evidence = pinned, digest_bytes(pinned), packages.REVIEW
            with patch.object(gate, "load_package_evidence", return_value=evidence):
                output = self.root / "wrong-scope"
                with self.assertRaises(gate.ScanRefusal) as refused:
                    gate.verified_staging_scan(archive, output, component="runtime-omarchy")
            self.assertEqual(refused.exception.outcome, "content_refused")
            report = json.loads((output / "REPORT.json").read_bytes())
            self.assertEqual(report["counts"]["blocking"], 1)
            self.assertNotIn("package_evidence", report)
