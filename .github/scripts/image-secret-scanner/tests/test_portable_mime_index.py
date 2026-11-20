"""Prove a generated Omarchy MIME index from the signed sources visible at its own layer."""

import copy
import subprocess
import sys
import tarfile
import unittest
from unittest.mock import patch

from image_secrets import mime_markers, mime_sources, package_receipt, reviewed_literals
from image_secrets.common import ScanError, digest_bytes
from image_secrets.gate import FORMAT_MARKER_ADVISORY, KEY_MARKER_VIEW_ADVISORY, KEY_MARKER_VIEW_RULE
from image_secrets.package_evidence import OMARCHY_PORTABLE_SCHEMA
from image_secrets.package_occurrences import prove_package_member
from tests import fixtures
from tests import test_derived_payload as derived_tests
from tests import test_key_marker_view as key_tests
from tests import test_package_policy as packages

MAGIC = "usr/share/mime/magic"
INPUT = mime_sources.PACKAGES + "/io.example.xml"
DOCKERFILE = "api/crates/layer4-runtime/Dockerfile"
ROOT = "var/lib/pacman/local"
PKCS8 = mime_markers.DISCRIMINATORS[b"application/pkcs8"]
ENCRYPTED = mime_markers.DISCRIMINATORS[b"application/pkcs8-encrypted"]
GENERATOR = b"\x7fELF fixture update-mime-database\n"
HOOK = b"[Action]\nExec = /usr/bin/update-mime-database /usr/share/mime\n"
SIGNED_XML = b"<mime-info><mime-type type=\"application/pkcs8\"/></mime-info>\n"
EXAMPLE_XML = b"<mime-info><mime-type type=\"text/x-example\"/></mime-info>\n"
MACHINE = [("etc/machine-id", b""), ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id"))]
SMI = ("shared-mime-info", "2.5.1-2")
BASE = ("example-base", "1.0-1")


CLOSING = b"-----END " + b"PRIVATE KEY-----"
# A long section name lets the detector reach a closing label that is not a discriminator.
UNRELATED = b"application/x-" + b"published-example-" * 3 + b"format"


def magic_index(second=b"application/pkcs8-encrypted", final=ENCRYPTED):
    record = b"[50:application/pkcs8]\n>0=" + len(PKCS8).to_bytes(2, "big") + PKCS8 + b"\n"
    following = b"[50:" + second + b"]\n>0=" + len(final).to_bytes(2, "big") + final + b"\n"
    return mime_markers.MAGIC_HEADER + record + following


def pacman(name, version, files, *, listed=None):
    label = f"{ROOT}/{name}-{version}"
    mtree = b"#mtree\n" + b"".join(f"./{path} type=file sha256digest={digest_bytes(data)}\n".encode()
                                   for path, data in files)
    names = [path for path, _ in files] if listed is None else listed
    return [(f"{label}/desc", f"%NAME%\n{name}\n\n%VERSION%\n{version}\n\n%ARCH%\nx86_64\n".encode()),
            (f"{label}/files", ("%FILES%\n" + "".join(path + "\n" for path in names)).encode()),
            (f"{label}/mtree", mtree)]


class PortableMimeIndexTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    detector_scan = packages.PackageTests.detector_scan
    policy = staticmethod(packages.PackageTests.policy)
    publish = derived_tests.DerivedPayloadTests.publish

    def image(self, name, *, example=EXAMPLE_XML, recorded=None, extra=(), late=(), smi_version=SMI[1],
              base_listed=None, smi_listed=None, generator=GENERATOR, output=None, padding=False):
        smi_files = [(mime_sources.SOURCE, SIGNED_XML), (mime_sources.GENERATOR, generator), (mime_sources.HOOK, HOOK)]
        base_files = [(INPUT, example)] if example is not None else []
        # ``recorded`` lets the native database disagree with the bytes actually written.
        database = [(INPUT, recorded or example)] if example is not None else []
        lower = [(f"{ROOT}/ALPM_DB_VERSION", b"9\n"), *pacman(*BASE, database, listed=base_listed), *base_files]
        if extra:
            lower += [*pacman("example-extra", "1.0-1", list(extra)), *extra]
        upper = [*pacman(SMI[0], smi_version, smi_files, listed=smi_listed), *smi_files, *late,
                 (MAGIC, magic_index() if output is None else output), *MACHINE]
        layers = [fixtures.tar_bytes(lower), fixtures.tar_bytes(upper, modes={mime_sources.GENERATOR: 0o755})]
        if padding:
            layers.insert(0, fixtures.tar_bytes([("padding", b"ordinary")]))
        archive = self.root / (name + ".tar")
        (fixtures.oci_archive if padding else fixtures.docker_archive)(archive, layers)
        return archive

    def vendor(self, name, members):
        payload = fixtures.tar_bytes(members)
        artifact = self.root / f"vendor-{name}"
        artifact.write_bytes(payload)
        trusted = digest_bytes(payload)
        package = {"name": name, "version": dict((SMI, BASE))[name], "architecture": "x86_64",
                   "artifact_sha256": trusted, "authority_type": "arch-signature", "authority_sha256": trusted}
        proved = {}
        for index, (path, _) in enumerate(members):
            proved[path] = prove_package_member(artifact, trusted, str(index), self.root)
        return package, proved

    def catalog(self, raw, data=None, final=ENCRYPTED):
        smi_files = [(mime_sources.SOURCE, SIGNED_XML), (mime_sources.GENERATOR, GENERATOR), (mime_sources.HOOK, HOOK)]
        smi, smi_members = self.vendor(SMI[0], smi_files)
        base, base_members = self.vendor(BASE[0], [(INPUT, EXAMPLE_XML)])
        mtrees = {name: digest_bytes(entry[1]) for name, entries in (
            (SMI[0], pacman(*SMI, smi_files)), (BASE[0], pacman(*BASE, [(INPUT, EXAMPLE_XML)])))
            for entry in entries if entry[0].endswith("/mtree")}
        sources = sorted(({"path": path, "sha256": member["sha256"], "package_sha256": owner["artifact_sha256"],
                           "package_member": member, "mtree_sha256": mtrees[owner["name"]]}
                          for owner, members in ((smi, smi_members), (base, base_members))
                          for path, member in members.items()), key=lambda source: source["path"])
        data = magic_index() if data is None else data
        [marker] = [row for row in raw["blocking_findings"] if row["rule"] == "content.private-key-header"]
        [found] = [row for row in raw["blocking_findings"] if row["rule"] == KEY_MARKER_VIEW_RULE]
        start, end = data.index(PKCS8), data.index(final) + len(final)
        window = {"offset": start, "size": end - start, "sha256": found["candidate_sha256"]}
        self.assertEqual(digest_bytes(data[start:end]), found["candidate_sha256"])
        middle = data[start + len(PKCS8):end - len(final)]
        spans = [{"offset": begin, "size": stop - begin, "sha256": digest_bytes(data[begin:stop]),
                  "context": {"offset": 0, "size": len(data), "sha256": digest_bytes(data)},
                  "source": {"path": mime_sources.SOURCE, "line": 1, "sha256": digest_bytes(data[begin:stop])}}
                 for begin, stop, _ in reviewed_literals.occurrences(data, marker["rule"])]
        producer = {key + "_sha256": digest_bytes(key.encode()) for key in ("attestation", "recipe", "toolchain")}
        producer.update(identity_sha256=mime_sources.identity_sha256(smi), inputs_sha256=mime_sources.inputs_sha256(sources))
        reserved = self.root / "reserved-source"
        recipe = reserved / DOCKERFILE
        recipe.parent.mkdir(parents=True, exist_ok=True)
        recipe.write_bytes(b"reviewed producer recipe")
        row = {
            "component": "runtime-omarchy", "platform": "linux/amd64", "path": MAGIC, "file_sha256": digest_bytes(data),
            "package_sha256": smi["artifact_sha256"], "package_member": dict(smi_members[mime_sources.SOURCE]),
            "member": None, "occurrences": 1, "ownership": {"build_inputs": {DOCKERFILE: digest_bytes(recipe.read_bytes())}},
            "derived_payload": {"kind": "mime-index", "input_sha256": digest_bytes(SIGNED_XML),
                                "output_sha256": digest_bytes(data), "producer": producer, "sources": sources},
            "reviewed_literals": {"findings": [{"rule": marker["rule"], "spans": spans}]},
            "key_marker_view": {"matches": [{"original": window, "detector": dict(window), "parts": [
                {"kind": kind, "size": len(chunk), "sha256": digest_bytes(chunk), "source": mime_sources.SOURCE,
                 "line": 1, "proof_sha256": digest_bytes(chunk)}
                for kind, chunk in (("begin", PKCS8), ("records", middle), ("end", final))]}]},
            "matches": [{"rule": marker["rule"], "candidate_sha256": None, "line": None,
                         "explanation": FORMAT_MARKER_ADVISORY, "proof_sha256": digest_bytes(b"marker")},
                        {"rule": found["rule"], "candidate_sha256": found["candidate_sha256"], "line": found["line"],
                         "explanation": KEY_MARKER_VIEW_ADVISORY, "proof_sha256": digest_bytes(b"records")}],
        }
        catalog = {"schema": OMARCHY_PORTABLE_SCHEMA, "scanner_manifest_sha256": packages.SCANNER,
                   "packages": [smi, base], "entries": [row]}
        return catalog, reserved

    def accepted(self):
        archive = self.image("generated")
        raw = self.detector_scan(archive, "runtime-omarchy")
        self.assertEqual(sorted(row["rule"] for row in raw["blocking_findings"]),
                         ["content.private-key-header", KEY_MARKER_VIEW_RULE])
        catalog, reserved = self.catalog(raw)
        return archive, raw, catalog, reserved

    def assert_blocked(self, archive, catalog, reserved, name):
        refused = self.publish(archive, catalog, name, blocks=True, source=reserved)
        self.assertEqual(refused["package_evidence"]["entries"], [])
        self.assertEqual(len(refused["blocking_findings"]), 2)

    def test_signed_sources_prove_rebuilt_index_and_its_detector_view(self):
        archive, raw, catalog, reserved = self.accepted()
        passed = self.publish(archive, catalog, "accepted", source=reserved)
        self.assertEqual(passed["findings"], raw["findings"])
        rows = passed["package_evidence"]["entries"]
        self.assertEqual({row["classification"] for row in rows}, {FORMAT_MARKER_ADVISORY, KEY_MARKER_VIEW_ADVISORY})
        for row in rows:
            owner = row["current_owner"]
            self.assertEqual([(source["path"], source["layer"]) for source in owner["sources"]],
                             [(mime_sources.GENERATOR, 1), (mime_sources.HOOK, 1), (mime_sources.SOURCE, 1), (INPUT, 0)])
            self.assertEqual(row["derived_payload"]["findings"], [row["finding_id"]])
            self.assertNotEqual(row["package_member"]["sha256"], row["outer_file_sha256"])
        rebuilt = self.image("rebuilt", padding=True)
        moved = self.publish(rebuilt, catalog, "rebuilt-accepted", source=reserved)
        self.assertEqual([source["layer"] for source in moved["package_evidence"]["entries"][0]["current_owner"]["sources"]],
                         [2, 2, 2, 1])
        self.assertNotEqual(moved["archive_sha256"], passed["archive_sha256"])

    def test_changed_missing_or_unowned_sources_keep_the_index_blocking(self):
        archive, _, catalog, reserved = self.accepted()
        self.publish(archive, catalog, "baseline", source=reserved)
        extra = (mime_sources.PACKAGES + "/extra.xml", b"<mime-info/>\n")
        changed = b"<mime-info><changed/></mime-info>\n"
        cases = {
            "changed-input": {"example": changed},
            "tampered-input": {"example": changed, "recorded": EXAMPLE_XML},
            "missing-input": {"example": None},
            "extra-owned-input": {"extra": [extra]},
            "unowned-input": {"base_listed": []},
            "overlay-input": {"late": [(INPUT, EXAMPLE_XML)]},
            "disowned-later": {"late": [(f"{ROOT}/{BASE[0]}-{BASE[1]}/files", b"%FILES%\n")]},
            "whiteout-input": {"late": [(mime_sources.PACKAGES + "/.wh.io.example.xml", b"")]},
            "symlink-input": {"late": [(mime_sources.PACKAGES + "/link.xml", (tarfile.SYMTYPE, "io.example.xml"))]},
            "changed-version": {"smi_version": "2.5.1-3"},
            "changed-generator": {"generator": GENERATOR + b"changed"},
            "claimed-output": {"smi_listed": [mime_sources.SOURCE, mime_sources.GENERATOR, mime_sources.HOOK, MAGIC]},
        }
        for name, change in cases.items():
            with self.subTest(case=name):
                self.assert_blocked(self.image(name, **change), catalog, reserved, name)

    def test_reserved_recipe_output_and_independent_key_material_refuse(self):
        archive, _, catalog, reserved = self.accepted()
        (reserved / DOCKERFILE).write_bytes(b"changed producer recipe")
        self.assert_blocked(archive, catalog, reserved, "changed-recipe")
        (reserved / DOCKERFILE).write_bytes(b"reviewed producer recipe")
        key = subprocess.run(["openssl", "genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:prime256v1",
                              "-outform", "DER"], capture_output=True, check=True, timeout=10).stdout
        changed = magic_index() + key
        pinned = copy.deepcopy(catalog)
        [row] = pinned["entries"]
        row["file_sha256"] = row["derived_payload"]["output_sha256"] = digest_bytes(changed)
        refused = self.publish(self.image("key-material", output=changed), pinned, "key-material", blocks=True,
                               source=reserved)
        self.assertEqual(refused["package_evidence"]["entries"], [])
        self.assertIn("content.binary-private-key", {row["rule"] for row in refused["blocking_findings"]})
        drifted = self.image("drifted-output", output=magic_index() + b"[50:text/x-other]\n")
        refused = self.publish(drifted, catalog, "drifted-output", blocks=True, source=reserved)
        self.assertEqual(refused["package_evidence"]["entries"], [])

    def test_detector_view_frames_must_be_published_discriminator_records(self):
        data = magic_index(UNRELATED, CLOSING)
        archive = self.image("closing-label", output=data)
        raw = self.detector_scan(archive, "runtime-omarchy")
        catalog, reserved = self.catalog(raw, data, CLOSING)
        refused = self.publish(archive, catalog, "closing-label", blocks=True, source=reserved)
        self.assertEqual([row["rule"] for row in refused["blocking_findings"]], [KEY_MARKER_VIEW_RULE])
        self.assertEqual([row["classification"] for row in refused["package_evidence"]["entries"]],
                         [FORMAT_MARKER_ADVISORY])

    def test_catalog_refuses_wrong_shape_path_or_producer(self):
        _, _, catalog, _ = self.accepted()
        self.policy(catalog)

        def changed(edit):
            altered = copy.deepcopy(catalog)
            edit(altered["entries"][0], altered)
            return altered

        sources = lambda row: row["derived_payload"]["sources"]  # noqa: E731
        edits = {
            "wrong-path": lambda row, _: row.update(path="usr/share/mime/globs2"),
            "inputs-digest": lambda row, _: row["derived_payload"]["producer"].update(inputs_sha256="0" * 64),
            "identity-digest": lambda row, _: row["derived_payload"]["producer"].update(identity_sha256="0" * 64),
            "no-generator": lambda row, _: sources(row).pop(0),
            "source-bytes": lambda row, _: sources(row)[-1].update(sha256="0" * 64),
            "unsigned-source": lambda _, value: value["packages"][1].update(authority_type="pinned-upstream"),
            "unsorted": lambda row, _: sources(row).reverse(),
            "signed-member": lambda row, _: row["package_member"].update(locator="1"),
            "mtree-ownership": lambda row, _: row["ownership"].update(mtree_sha256="0" * 64),
            "no-sources": lambda row, _: row["derived_payload"].pop("sources"),
            "compiled-kind": lambda row, _: row["derived_payload"].update(kind="compiled-executable"),
            "underived-view": lambda row, _: row.pop("derived_payload"),
        }
        for name, edit in edits.items():
            with self.subTest(case=name), self.assertRaises(ScanError):
                self.policy(changed(edit))

    def test_final_gate_requires_both_links_and_exclusive_receipts(self):
        archive, _, catalog, reserved = self.accepted()
        passed = self.publish(archive, catalog, "gate", source=reserved)
        with patch.object(sys, "path", [str(key_tests.SCRIPTS), *sys.path]):
            import image_secret_gate as gate

            def check(edit):
                report = copy.deepcopy(passed)
                section = report["package_evidence"]
                edit(section["entries"])
                report["policy"]["package_evidence_sha256"] = package_receipt.digest(section)
                gate.validate_receipt(report, report["archive_sha256"], "credential", None,
                                      component="runtime-omarchy", package_section=section)

            check(lambda rows: None)
            view = lambda rows: next(row for row in rows if "key_marker_view" in row)  # noqa: E731
            literal = lambda rows: next(row for row in rows if "reviewed_literal" in row)  # noqa: E731
            edits = {
                "source-drift": lambda rows: rows[0]["current_owner"]["sources"][-1].update(sha256="0" * 64),
                "input-link": lambda rows: rows[0]["derived_payload"].update(input_sha256="0" * 64),
                "output-link": lambda rows: rows[0]["derived_payload"].update(output_sha256="0" * 64),
                "view-and-literal": lambda rows: view(rows).update(reviewed_literal=literal(rows)["reviewed_literal"]),
                "literal-and-view": lambda rows: literal(rows).update(key_marker_view=view(rows)["key_marker_view"]),
            }
            for name, edit in edits.items():
                with self.subTest(case=name), self.assertRaises(gate.ScanRefusal):
                    check(edit)


class VisibleSourcesTests(unittest.TestCase):
    def replay(self, layers):
        visible = mime_sources.VisibleSources()
        for layer, entries in enumerate(layers):
            for entry, (path, kind) in enumerate(entries):
                member = tarfile.TarInfo(path)
                member.type = kind
                if kind == tarfile.DIRTYPE and path.endswith("!opaque"):
                    member.name = path = path.removesuffix("!opaque")
                    member.pax_headers = {"SCHILY.xattr.trusted.overlay.opaque": "y"}
                visible.observe(member, path, (0, layer, entry))
        return visible.visible((0, len(layers) - 1))

    def test_namespace_replay_applies_whiteouts_and_refuses_ambiguity(self):
        regular = tarfile.REGTYPE
        first = [(INPUT, regular), (mime_sources.SOURCE, regular), (mime_sources.GENERATOR, regular)]
        self.assertEqual(set(self.replay([first])), {INPUT, mime_sources.SOURCE, mime_sources.GENERATOR})
        deleted = self.replay([first, [(mime_sources.PACKAGES + "/.wh.io.example.xml", regular)]])
        self.assertNotIn(INPUT, deleted)
        cleared = self.replay([first, [(mime_sources.PACKAGES + "!opaque", tarfile.DIRTYPE),
                                       (mime_sources.SOURCE, regular)]])
        self.assertEqual(set(cleared), {mime_sources.SOURCE, mime_sources.GENERATOR})
        self.assertEqual(cleared[mime_sources.SOURCE][0], 1)
        for refused in ([(mime_sources.PACKAGES + "/link.xml", tarfile.SYMTYPE)],
                        [(mime_sources.PACKAGES + "/notes.txt", regular)],
                        [("usr/share/mime", tarfile.SYMTYPE)],
                        [("usr/.wh..wh..opq", regular)],
                        [(INPUT, regular), (INPUT, regular)]):
            with self.subTest(entries=refused):
                self.assertIsNone(self.replay([first, refused]))


if __name__ == "__main__":
    unittest.main()
