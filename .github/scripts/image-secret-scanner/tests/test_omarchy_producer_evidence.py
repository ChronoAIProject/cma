"""Prove CMA's pinned producer files in Omarchy by reserved build input, never by pacman."""

import copy
import json
import sys
import tarfile
import unittest
from pathlib import Path
from unittest.mock import patch

from image_secrets import derived_payload
from image_secrets.common import ScanError, digest_bytes
from image_secrets.package_evidence import ARCH_COMPILED_INPUTS, OMARCHY_PORTABLE_SCHEMA
from image_secrets.package_receipt import digest
from tests import fixtures
from tests import test_derived_payload as derived
from tests import test_package_policy as packages

SCRIPTS = Path(__file__).resolve().parents[2]
COMPONENT = "runtime-omarchy"
DOCKERFILE = "api/crates/layer4-runtime/Dockerfile"
ROOT = "var/lib/pacman/local"
IDENTITY = [("etc/machine-id", b""), ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id"))]


def portable(manifest, ownership):
    catalog = copy.deepcopy(manifest)
    catalog["schema"] = OMARCHY_PORTABLE_SCHEMA
    rows = {}
    for row in catalog["entries"]:
        for field in ("layer", "diff_id", "entry"):
            del row[field]
        origin = row.get("derived_payload")
        for field in ("archive_sha256", "image_config", "image_manifest", "findings"):
            (origin or {}).pop(field, None)
        key = (row["path"], row["file_sha256"])
        if key in rows:
            rows[key]["occurrences"] += 1
        else:
            rows[key] = dict(row, occurrences=1, ownership=copy.deepcopy(ownership))
    catalog["entries"] = list(rows.values())
    return catalog


def signed_package(path, contents, version="1.0"):
    """A pacman database whose mtree records the file, as the signed Arch rows require."""
    mtree = f"#mtree\n./{path} type=file sha256digest={digest_bytes(contents)}\n".encode()
    directory = f"{ROOT}/example-signed-{version}"
    desc = f"%NAME%\nexample-signed\n\n%VERSION%\n{version}\n\n%ARCH%\nx86_64\n".encode()
    entries = [(f"{ROOT}/ALPM_DB_VERSION", b"9\n"), (f"{directory}/desc", desc),
               (f"{directory}/files", f"%FILES%\n{path}\n".encode()), (f"{directory}/mtree", mtree)]
    return entries, mtree


class OmarchyProducerTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    manifest = packages.PackageTests.manifest
    policy = staticmethod(packages.PackageTests.policy)
    detector_scan = packages.PackageTests.detector_scan
    fixture = derived.DerivedPayloadTests.fixture
    binding = derived.DerivedPayloadTests.binding
    publish = derived.DerivedPayloadTests.publish

    def reserved(self, names, content=b"reviewed producer input"):
        source = self.root / "reserved-source"
        inputs = {}
        for name in sorted(names):
            path = source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content + name.encode())
            inputs[name] = digest_bytes(path.read_bytes())
        return source, inputs

    def producer(self, body=packages.PUBLIC_SOURCE):
        vendor = fixtures.tar_bytes([(packages.SOURCE, packages.PUBLIC_SOURCE)])
        source, inputs = self.reserved([DOCKERFILE])
        reference = self.manifest(vendor, vendor, component=COMPONENT)
        catalog = portable(reference, {"build_inputs": inputs})
        archive = self.root / "producer.tar"
        fixtures.oci_archive(archive, [fixtures.tar_bytes([("padding", b"ordinary")]),
                                       fixtures.tar_bytes([(packages.SOURCE, body), *IDENTITY])])
        return archive, catalog, source

    def test_pinned_producer_row_is_proved_by_reserved_inputs(self):
        archive, catalog, source = self.producer()
        accepted = self.publish(archive, catalog, "producer", source=source)
        [receipt] = accepted["package_evidence"]["entries"]
        self.assertEqual(receipt["layer"], 1)
        expected = catalog["entries"][0]["ownership"]["build_inputs"]
        self.assertEqual(receipt["current_owner"], {"build_inputs": expected})
        self.assertEqual(receipt["package"]["authority_type"], "pinned-upstream")
        self.assertEqual(accepted["counts"]["blocking"], 0)

        (source / DOCKERFILE).write_bytes(b"changed producer input")
        changed = self.publish(archive, catalog, "changed-input", blocks=True, source=source)
        self.assertEqual(changed["package_evidence"]["entries"], [])
        unsourced = self.publish(archive, catalog, "missing-source", blocks=True)
        self.assertEqual(unsourced["package_evidence"]["entries"], [])

    def test_changed_bytes_wrong_path_copies_and_keys_stay_blocking(self):
        archive, catalog, source = self.producer(packages.PUBLIC_SOURCE + b"// changed upstream\n")
        changed = self.publish(archive, catalog, "changed-bytes", blocks=True, source=source)
        self.assertEqual(changed["package_evidence"]["entries"], [])

        archive, catalog, source = self.producer()
        moved = copy.deepcopy(catalog)
        moved["entries"][0]["path"] = "usr/local/lib/example/parser.js"
        refused = self.publish(archive, moved, "wrong-path", blocks=True, source=source)
        self.assertEqual(refused["package_evidence"]["entries"], [])

        copied = self.root / "app-copy.tar"
        layer = fixtures.tar_bytes([(packages.SOURCE, packages.PUBLIC_SOURCE), *IDENTITY])
        again = fixtures.tar_bytes([("added", b"x"), (packages.SOURCE, packages.PUBLIC_SOURCE)])
        fixtures.oci_archive(copied, [layer, again])
        duplicate = self.publish(copied, catalog, "unexplained-copy", blocks=True, source=source)
        self.assertEqual(duplicate["counts"]["blocking"], 2)
        self.assertEqual(duplicate["package_evidence"]["entries"], [])

        keyed = self.root / "extra-key.tar"
        fixtures.oci_archive(keyed, [fixtures.tar_bytes([(packages.SOURCE, packages.PUBLIC_SOURCE),
                                                         ("usr/lib/example/material", fixtures.fake_key()),
                                                         *IDENTITY])])
        mixed = self.publish(keyed, catalog, "extra-key", blocks=True, source=source)
        self.assertEqual([row["rule"] for row in mixed["package_evidence"]["entries"]], [packages.RULE])
        self.assertIn("content.private-key-header", {row["rule"] for row in mixed["blocking_findings"]})

    def test_signed_and_producer_owners_are_not_interchangeable(self):
        archive, catalog, source = self.producer()
        [row] = catalog["entries"]
        entries, mtree = signed_package(packages.SOURCE, packages.PUBLIC_SOURCE)
        both = {**row["ownership"], "mtree_sha256": "e" * 64}
        for ownership in ({"mtree_sha256": digest_bytes(mtree)}, both, {}):
            with self.subTest(ownership=sorted(ownership)):
                altered = copy.deepcopy(catalog)
                altered["entries"][0]["ownership"] = ownership
                with self.assertRaises(ScanError):
                    self.policy(altered)
        signed = copy.deepcopy(catalog)
        signed["packages"][0].update(authority_type="arch-signature", architecture="x86_64", name="example-signed")
        with self.assertRaises(ScanError):
            self.policy(signed)
        signed["entries"][0]["ownership"] = {"mtree_sha256": digest_bytes(mtree)}
        self.policy(signed)
        # A pacman record for the same bytes cannot stand in for missing reserved inputs.
        fixtures.oci_archive(archive, [fixtures.tar_bytes([*entries, (packages.SOURCE, packages.PUBLIC_SOURCE),
                                                           *IDENTITY])])
        self.publish(archive, signed, "signed-owner")
        refused = self.publish(archive, catalog, "pacman-for-producer", blocks=True)
        self.assertEqual(refused["package_evidence"]["entries"], [])
        changed = copy.deepcopy(signed)
        changed["packages"][0]["version"] = "1.1"
        refused = self.publish(archive, changed, "changed-version", blocks=True)
        self.assertEqual(refused["package_evidence"]["entries"], [])
        for view in ("self_test", "key_marker_view", "distribution_config", "debian_control"):
            with self.subTest(view=view):
                altered = copy.deepcopy(catalog)
                altered["entries"][0][view] = {}
                with self.assertRaises(ScanError):
                    self.policy(altered)

    def compiled(self, name, *, copies=2):
        prefix = derived.executable.ELF + b"\0\nFIPSKEY="
        contents = prefix + derived.CONSTANT + b"\0\n"
        recipe = ("Configure", b"fixture build recipe")
        archive, manifest, report, layers = self.fixture(name, derived.OPENSSL, recipe, contents,
                                                         component=COMPONENT, copies=copies)
        manifest["packages"][0]["name"] = "openssl"
        [base] = manifest["entries"]
        manifest["entries"] = []
        for index, layer in enumerate(layers):
            [finding] = [row for row in report["blocking_findings"]
                         if row["rule"] == derived_payload.COMPILED_RULE and row["layer"] == index]
            row = dict(copy.deepcopy(base), path=derived.OPENSSL, layer=index,
                       diff_id="sha256:" + digest_bytes(layer))
            row["matches"] = [{"rule": finding["rule"], "candidate_sha256": finding["candidate_sha256"],
                               "line": None, "explanation": "public-noncredential-constant",
                               "proof_sha256": digest_bytes(b"reviewed source line")}]
            proof = self.binding(row, report, finding, "compiled-executable", contents)
            proof["span"] = {"offset": len(prefix), "size": len(derived.CONSTANT),
                             "sha256": digest_bytes(derived.CONSTANT)}
            manifest["entries"].append(row)
        source, inputs = self.reserved(ARCH_COMPILED_INPUTS)
        return archive, portable(manifest, {"build_inputs": inputs}), source

    def test_rebuilt_arch_cli_binds_the_exact_arch_recipe_in_both_layers(self):
        archive, catalog, source = self.compiled("arch-cli")
        accepted = self.publish(archive, catalog, "arch-cli", source=source)
        rows = accepted["package_evidence"]["entries"]
        self.assertEqual([row["layer"] for row in rows], [0, 1])
        for row in rows:
            self.assertEqual(set(row["current_owner"]), {"build_inputs"})
            self.assertEqual(row["derived_payload"]["findings"], [row["finding_id"]])
            self.assertEqual(row["derived_payload"]["archive_sha256"], accepted["archive_sha256"])

        inputs = catalog["entries"][0]["ownership"]["build_inputs"]
        debian = dict(inputs)
        debian["api/crates/layer4-runtime/image/build-debian-openssl-cli.sh"] = debian.pop(
            "api/crates/layer4-runtime/image/build-arch-openssl-cli.sh")
        native = dict(inputs)
        native.pop(DOCKERFILE)
        for ownership in ({"build_inputs": debian}, {"build_inputs": native},
                          {"build_inputs": inputs, "debian_owner": {"name": "openssl", "version": "1.0-1",
                                                                   "architecture": "amd64"}}):
            with self.subTest(ownership=sorted(ownership["build_inputs"]) + sorted(ownership)):
                altered = copy.deepcopy(catalog)
                altered["entries"][0]["ownership"] = ownership
                with self.assertRaises(ScanError):
                    self.policy(altered)

        shifted = copy.deepcopy(catalog)
        shifted["entries"][0]["derived_payload"]["span"]["offset"] += 1
        refused = self.publish(archive, shifted, "arch-cli-span", blocks=True, source=source)
        self.assertEqual(refused["counts"]["blocking"], 2)
        single = copy.deepcopy(catalog)
        single["entries"][0]["occurrences"] = 1
        refused = self.publish(archive, single, "arch-cli-one-copy", blocks=True, source=source)
        self.assertEqual(refused["package_evidence"]["entries"], [])
        (source / "api/crates/layer4-runtime/image/build-arch-openssl-cli.sh").write_bytes(b"other recipe")
        refused = self.publish(archive, catalog, "arch-cli-recipe", blocks=True, source=source)
        self.assertEqual(refused["package_evidence"]["entries"], [])

    def test_final_receipt_requires_the_owner_kind_of_each_package(self):
        archive, catalog, source = self.producer()
        report = self.publish(archive, catalog, "receipt", source=source)
        with patch.object(sys, "path", [str(SCRIPTS), *sys.path]):
            import image_secret_gate as gate

            def check(section):
                altered = copy.deepcopy(report)
                altered["package_evidence"] = section
                altered["policy"]["package_evidence_sha256"] = digest(section)
                gate.validate_receipt(altered, report["archive_sha256"], "credential", None,
                                      component=COMPONENT, package_section=section)

            check(report["package_evidence"])
            for owner in ({"pacman": {}}, {}, {"build_inputs": {"elsewhere/Dockerfile": "a" * 64}}):
                with self.subTest(owner=json.dumps(owner)):
                    section = copy.deepcopy(report["package_evidence"])
                    section["entries"][0]["current_owner"] = owner
                    with self.assertRaises(gate.ScanRefusal):
                        check(section)


if __name__ == "__main__":
    unittest.main()
