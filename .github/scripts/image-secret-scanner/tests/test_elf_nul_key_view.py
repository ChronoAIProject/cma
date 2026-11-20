"""Prove all three reviewed ELF declarations through the publication gate."""

import copy
import struct
import subprocess
import tarfile
import unittest
from unittest.mock import patch

from image_secrets import elf_nul_key_view
from image_secrets.arch_package_ownership import ROOT, VERSION
from image_secrets.binary_view import TEXT_BYTES
from image_secrets.common import digest_bytes
from image_secrets.gate import FORMAT_MARKER_ADVISORY, KEY_MARKER_VIEW_ADVISORY, KEY_MARKER_VIEW_RULE
from image_secrets.package_evidence import OMARCHY_PORTABLE_SCHEMA
from tests import fixtures
from tests import test_derived_payload as derived_tests
from tests import test_key_marker_view as key_tests
from tests import test_package_policy as packages

PATH = elf_nul_key_view.PATHS["libssh2"]
QT_PATH = elf_nul_key_view.PATHS["qt6-base"]
MIDDLE = b"parser recognizes published header declarations " + b"." * 65
LABELS = (b"-----END " + b"PUBLIC KEY-----", b"-----END RSA " + b"PRIVATE KEY-----")
OFFSET = 256


def elf(body, *, writable=False):
    header = b"\x7fELF\x02\x01\x01" + b"\0" * 9
    header += struct.pack("<HHIQQQIHHHHHH", 3, 62, 1, 0, 64, 0, 0, 64, 56, 1, 0, 0, 0)
    size = OFFSET + len(body)
    segment = struct.pack("<IIQQQQQQ", 1, 6 if writable else 4, 0, 0x400000, 0, size, size, 4096)
    return (header + segment).ljust(OFFSET, b"\0") + body


class ElfNulKeyViewTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    detector_scan = packages.PackageTests.detector_scan
    manifest = packages.PackageTests.manifest
    publish = derived_tests.DerivedPayloadTests.publish

    def fixture(self, *, gap=b"\0\0", writable=False, duplicate=False, middle=MIDDLE, extra=b"", inner=None,
                path=PATH, package="libssh2"):
        declarations = (key_tests.BEGIN, *(inner or (middle,)), key_tests.END)
        body = gap.join(declarations)
        alternate = b"\0" + (b"\x03" * len(gap)).join(declarations) if duplicate else b""
        contents = elf(body + alternate + extra, writable=writable)
        supplied = fixtures.tar_bytes([(path, contents)])
        layer = fixtures.tar_bytes([(path, contents), ("etc/machine-id", b""),
                                    ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id"))])
        archive = self.root / "ssh-view.tar"
        fixtures.oci_archive(archive, [layer])
        scope = "runtime-omarchy"
        raw = self.detector_scan(archive, scope)
        [image] = raw["inventory"]["images"]
        marker = next(row for row in raw["blocking_findings"] if row["rule"] == "content.private-key-header")
        detector = body.translate(TEXT_BYTES)
        leak = next(row for row in raw["blocking_findings"] if row["rule"] == KEY_MARKER_VIEW_RULE
                    and row["candidate_sha256"] == digest_bytes(detector))
        manifest = self.manifest(supplied, layer, component=scope, file=path)
        manifest["packages"][0].update(name=package, authority_type="arch-signature", architecture="x86_64")
        [row] = manifest["entries"]
        source = {"path": "src/public-format-labels.c", "line": 1, "sha256": digest_bytes(body)}
        parts = []
        offset = OFFSET
        for index, declaration in enumerate(declarations, 1):
            parts.append({"offset": offset, "size": len(declaration), "sha256": digest_bytes(declaration),
                          "source": source["path"], "line": index, "proof_sha256": source["sha256"]})
            offset += len(declaration) + len(gap)
        identity = {"archive_sha256": raw["archive_sha256"], "image_config": image["config_digest"],
                    "image_manifest": image["manifest"]}
        row["elf_nul_key_view"] = {
            **identity, "encoding": elf_nul_key_view.ENCODING, "matches": [{
                "finding_id": leak["finding_id"], "declarations": parts,
                "original": {"offset": OFFSET, "size": len(body), "sha256": digest_bytes(body)},
                "detector": {"offset": OFFSET, "size": len(detector), "sha256": digest_bytes(detector)},
            }],
        }
        row["reviewed_literals"] = {
            **identity, "findings": [{
                "rule": marker["rule"], "finding_id": marker["finding_id"],
                "spans": [{"offset": OFFSET, "size": len(key_tests.BEGIN), "sha256": digest_bytes(key_tests.BEGIN),
                           "context": {"offset": OFFSET, "size": len(body), "sha256": digest_bytes(body)},
                           "source": source}],
            }],
        }
        row["matches"] = [
            {"rule": leak["rule"], "candidate_sha256": leak["candidate_sha256"], "line": leak.get("line"),
             "explanation": KEY_MARKER_VIEW_ADVISORY, "proof_sha256": source["sha256"]},
            {"rule": marker["rule"], "candidate_sha256": None, "line": None,
             "explanation": FORMAT_MARKER_ADVISORY, "proof_sha256": source["sha256"]},
        ]
        return archive, manifest, raw

    def test_whole_gate_replays_the_exact_view_with_cold_and_warm_caches(self):
        archive, manifest, raw = self.fixture()
        cache = self.root / "elf-layer-cache.json"
        options = ["--detector-cache-context", "e" * 64, "--detector-cache-output", str(cache)]
        cold = self.publish(archive, manifest, "elf-cold", options=options)
        warm = self.publish(archive, manifest, "elf-warm", options=[*options, "--detector-cache-input", str(cache)])
        self.assertEqual(warm["detector_cache"]["hits"], 1)
        self.assertEqual(cold["findings"], raw["findings"])
        self.assertEqual(cold["package_evidence"], warm["package_evidence"])
        [row] = [item for item in warm["package_evidence"]["entries"] if item["classification"] == KEY_MARKER_VIEW_ADVISORY]
        self.assertEqual(row["elf_nul_key_view"]["encoding"], elf_nul_key_view.ENCODING)

    def test_non_nul_gaps_writable_mappings_and_duplicate_views_stay_blocking(self):
        for name, options in (("bad-gap", {"gap": b"\0\x03"}), ("writable", {"writable": True}),
                              ("duplicate-view", {"duplicate": True})):
            with self.subTest(name=name):
                archive, manifest, raw = self.fixture(**options)
                refused = self.publish(archive, manifest, name, blocks=True)
                self.assertEqual(refused["findings"], raw["findings"])
                self.assertNotIn(KEY_MARKER_VIEW_ADVISORY,
                                 {item["classification"] for item in refused["package_evidence"]["entries"]})
                self.assertIn(KEY_MARKER_VIEW_RULE, {item["rule"] for item in refused["blocking_findings"]})

    def test_changed_declaration_leaves_the_matching_secret_rule_blocking(self):
        archive, manifest, raw = self.fixture()
        altered = copy.deepcopy(manifest)
        [item] = altered["entries"][0]["elf_nul_key_view"]["matches"]
        item["declarations"][1]["sha256"] = digest_bytes(b"unrelated declaration")
        refused = self.publish(archive, altered, "changed", blocks=True)
        self.assertEqual(refused["findings"], raw["findings"])
        self.assertEqual([row["rule"] for row in refused["blocking_findings"]], [KEY_MARKER_VIEW_RULE])

    def test_public_diagnostic_requires_the_reviewed_member_and_exact_declaration(self):
        middle = b"Unable to read the public fixture marker from input symbols"
        archive, manifest, raw = self.fixture(middle=middle)
        [row] = manifest["entries"]
        [claim] = row["elf_nul_key_view"]["matches"]
        diagnostic = claim["declarations"][1]
        spec = {"artifact_sha256": row["package_sha256"], "file_sha256": row["file_sha256"],
                "literal_sha256": diagnostic["sha256"], "source": diagnostic["source"],
                "source_sha256": diagnostic["proof_sha256"], "lines": (diagnostic["line"],),
                "version": manifest["packages"][0]["version"], "offset": OFFSET, "size": len(middle)}
        denied = self.publish(archive, manifest, "unreviewed-diagnostic", blocks=True)
        self.assertEqual([item["rule"] for item in denied["blocking_findings"]], [KEY_MARKER_VIEW_RULE])
        row["elf_nul_key_view"]["middle_diagnostic"] = elf_nul_key_view.DIAGNOSTIC
        with patch.multiple(elf_nul_key_view, LIBSSH2_DIAGNOSTIC=spec):
            clean = self.publish(archive, manifest, "reviewed-diagnostic")
            self.assertEqual(clean["findings"], raw["findings"])
            self.assertEqual(clean["blocking_findings"], [])
            wrong_path = copy.deepcopy(manifest)
            wrong_path["entries"][0]["path"] += ".other"
            self.assertIsNone(key_tests.KeyMarkerViewTests.refused(
                self, archive, wrong_path, "different-member", classification="content_refused"))
            wrong_offset = copy.deepcopy(manifest)
            [claim] = wrong_offset["entries"][0]["elf_nul_key_view"]["matches"]
            for span in (claim["original"], claim["detector"], *claim["declarations"]):
                span["offset"] += 1
            self.assertIsNone(key_tests.KeyMarkerViewTests.refused(
                self, archive, wrong_offset, "different-offset", classification="content_refused"))
            for name, changes in (("different-declaration", {"middle": middle + b"x"}),
                                  ("different-separator", {"gap": b"\0\x03"})):
                with self.subTest(name=name):
                    changed_archive, changed_manifest, _ = self.fixture(middle=changes.get("middle", middle),
                                                                         gap=changes.get("gap", b"\0\0"))
                    changed_manifest["entries"][0]["elf_nul_key_view"]["middle_diagnostic"] = elf_nul_key_view.DIAGNOSTIC
                    self.assertIsNone(key_tests.KeyMarkerViewTests.refused(
                        self, changed_archive, changed_manifest, name, classification="content_refused"))

        generated = subprocess.run(["openssl", "genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:prime256v1",
                                    "-outform", "DER"], capture_output=True, check=True, timeout=10).stdout
        archive, altered, raw = self.fixture(middle=middle, extra=generated)
        [row] = altered["entries"]
        row["elf_nul_key_view"]["middle_diagnostic"] = elf_nul_key_view.DIAGNOSTIC
        altered_reference = dict(spec, artifact_sha256=row["package_sha256"], file_sha256=row["file_sha256"])
        with patch.multiple(elf_nul_key_view, LIBSSH2_DIAGNOSTIC=altered_reference):
            refused = self.publish(archive, altered, "independent-key", blocks=True)
        self.assertEqual(refused["findings"], raw["findings"])
        self.assertIn("content.binary-private-key", {row["rule"] for row in refused["blocking_findings"]})
        self.assertNotIn(KEY_MARKER_VIEW_ADVISORY,
                         {row["classification"] for row in refused["package_evidence"]["entries"]})

    def qt_labels(self, **options):
        archive, manifest, raw = self.fixture(inner=options.pop("inner", LABELS), path=QT_PATH, package="qt6-base",
                                              **options)
        [row] = manifest["entries"]
        row["elf_nul_key_view"]["middle_labels"] = elf_nul_key_view.LABELS
        [claim] = row["elf_nul_key_view"]["matches"]
        parts = claim["declarations"]
        spec = {"artifact_sha256": row["package_sha256"], "file_sha256": row["file_sha256"],
                "source": parts[0]["source"], "source_sha256": parts[0]["proof_sha256"],
                "lines": tuple(part["line"] for part in parts), "version": manifest["packages"][0]["version"],
                "offset": OFFSET, "size": claim["original"]["size"]}
        return archive, manifest, raw, spec

    def test_qt_network_label_table_requires_the_pinned_member_and_published_labels(self):
        archive, manifest, raw, spec = self.qt_labels()
        unreviewed = copy.deepcopy(manifest)
        del unreviewed["entries"][0]["elf_nul_key_view"]["middle_labels"]
        self.assertIsNone(key_tests.KeyMarkerViewTests.refused(
            self, archive, unreviewed, "qt-unreviewed", classification="content_refused"))
        self.assertIsNone(key_tests.KeyMarkerViewTests.refused(
            self, archive, manifest, "qt-unpinned", classification="content_refused"))
        with patch.multiple(elf_nul_key_view, QT6_NETWORK_LABELS=spec):
            clean = self.publish(archive, manifest, "qt-labels")
            self.assertEqual(clean["findings"], raw["findings"])
            self.assertEqual(clean["blocking_findings"], [])
            [row] = [item for item in clean["package_evidence"]["entries"]
                     if item["classification"] == KEY_MARKER_VIEW_ADVISORY]
            self.assertEqual(row["elf_nul_key_view"]["middle_labels"], elf_nul_key_view.LABELS)
            changes = {
                "different-member": lambda value: value["entries"][0].update(path=QT_PATH + ".other"),
                "different-version": lambda value: value["packages"][0].update(version="6.12.0-4"),
                "different-package": lambda value: value["packages"][0].update(name="libssh2"),
                "reordered-lines": lambda value: value["entries"][0]["elf_nul_key_view"]["matches"][0][
                    "declarations"][1].update(line=spec["lines"][2]),
                "different-source": lambda value: value["entries"][0]["elf_nul_key_view"]["matches"][0][
                    "declarations"][2].update(source="src/network/ssl/other.cpp"),
                "missing-declaration": lambda value: value["entries"][0]["elf_nul_key_view"]["matches"][0][
                    "declarations"].pop(1),
                "diagnostic-flag": lambda value: value["entries"][0]["elf_nul_key_view"].update(
                    middle_diagnostic=elf_nul_key_view.DIAGNOSTIC),
            }
            for name, change in changes.items():
                with self.subTest(name=name):
                    altered = copy.deepcopy(manifest)
                    change(altered)
                    self.assertIsNone(key_tests.KeyMarkerViewTests.refused(
                        self, archive, altered, name, classification="content_refused"))
        for name, options in (("unlabelled-inner", {"inner": (LABELS[0], b"QTLSBACKENDPUBLICLABELTABLEROW")}),
                              ("different-separator", {"gap": b"\0\x03"})):
            with self.subTest(name=name):
                changed_archive, changed, changed_raw, changed_spec = self.qt_labels(**options)
                with patch.multiple(elf_nul_key_view, QT6_NETWORK_LABELS=changed_spec):
                    refused = self.publish(changed_archive, changed, name, blocks=True)
                self.assertEqual(refused["findings"], changed_raw["findings"])
                self.assertIn(KEY_MARKER_VIEW_RULE, {item["rule"] for item in refused["blocking_findings"]})

    def test_qt_network_labels_keep_independent_key_material_blocking(self):
        generated = subprocess.run(["openssl", "genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:prime256v1",
                                    "-outform", "DER"], capture_output=True, check=True, timeout=10).stdout
        archive, manifest, raw, spec = self.qt_labels(extra=generated)
        with patch.multiple(elf_nul_key_view, QT6_NETWORK_LABELS=spec):
            refused = self.publish(archive, manifest, "qt-independent-key", blocks=True)
        self.assertEqual(refused["findings"], raw["findings"])
        self.assertIn("content.binary-private-key", {row["rule"] for row in refused["blocking_findings"]})
        self.assertNotIn(KEY_MARKER_VIEW_ADVISORY,
                         {row["classification"] for row in refused["package_evidence"]["entries"]})

    def test_ssh_views_refuse_the_qt_label_flag(self):
        _, manifest, _ = self.fixture()
        archive = self.root / "ssh-view.tar"
        manifest["entries"][0]["elf_nul_key_view"]["middle_labels"] = elf_nul_key_view.LABELS
        self.assertIsNone(key_tests.KeyMarkerViewTests.refused(
            self, archive, manifest, "ssh-label-flag", classification="content_refused"))

    def portable(self):
        _, manifest, _ = self.fixture()
        manifest["schema"] = OMARCHY_PORTABLE_SCHEMA
        manifest["packages"][0]["version"] = "1:1.11.1-7"
        [row] = manifest["entries"]
        for field in ("layer", "diff_id", "entry"):
            del row[field]
        for field in ("elf_nul_key_view", "reviewed_literals"):
            proof = row[field]
            for key in ("archive_sha256", "image_config", "image_manifest"):
                del proof[key]
            del proof["matches" if field == "elf_nul_key_view" else "findings"][0]["finding_id"]
        mtree = f"#mtree\n./{PATH} type=file sha256digest={row['file_sha256']}\n".encode()
        row.update(occurrences=1, ownership={"mtree_sha256": digest_bytes(mtree)})
        body = elf(b"\0\0".join((key_tests.BEGIN, MIDDLE, key_tests.END)))
        return manifest, body, mtree

    def installed_package(self, name, version, mtree):
        directory = f"{ROOT}/{name}-{version}"
        desc = f"%NAME%\n{name}\n\n%VERSION%\n{version}\n\n%ARCH%\nx86_64\n"
        return [(f"{directory}/desc", desc.encode()),
                (f"{directory}/files", f"%FILES%\n{PATH}\n".encode()),
                (f"{directory}/mtree", mtree)]

    def archived(self, path, contents, database, *, inherited=False, earlier=False):
        identity = [("etc/machine-id", b""),
                    ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id"))]
        layers = []
        if inherited:
            layers = [fixtures.tar_bytes(database)]
            database = []
        elif earlier:
            layers = [fixtures.tar_bytes([("padding", b"ordinary")])]
        layers.append(fixtures.tar_bytes([*identity, *database, (PATH, contents)]))
        fixtures.oci_archive(path, layers)

    def test_portable_arch_receipt_rechecks_installed_version_and_unique_ownership(self):
        manifest, body, mtree = self.portable()
        version = "1:1.11.1-7"
        good = [(VERSION, b"9\n"), *self.installed_package("libssh2", version, mtree)]
        archive = self.root / "portable-ssh.tar"
        self.archived(archive, body, good)
        reference = self.publish(archive, manifest, "arch-reference")
        self.archived(archive, body, good, earlier=True)
        fresh = self.publish(archive, manifest, "arch-fresh")
        self.assertNotEqual(reference["archive_sha256"], fresh["archive_sha256"])
        for row in fresh["package_evidence"]["entries"]:
            self.assertEqual(row["layer"], 1)
            self.assertEqual(set(row["current_owner"]["pacman"]), {"desc", "files", "mtree"})
            self.assertEqual(row["current_owner"]["pacman"]["mtree"]["sha256"], digest_bytes(mtree))
        for name, records, inherited in (
            ("new-owner-version", [(VERSION, b"9\n"), *self.installed_package("libssh2", "1:1.11.1-8", mtree)], False),
            ("competing-owner", [*good, *self.installed_package("other", "1.0-1", mtree)], False),
            ("inherited-owner", good, True),
            ("changed-mtree", [*good[:-1], (good[-1][0], mtree + b"\n")], False),
        ):
            with self.subTest(name=name):
                self.archived(archive, body, records, inherited=inherited)
                refused = self.publish(archive, manifest, name, blocks=True)
                self.assertEqual(refused["package_evidence"]["entries"], [])
                self.assertEqual(refused["counts"]["blocking"], 2)
