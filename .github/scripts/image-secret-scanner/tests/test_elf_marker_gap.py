"""Prove pinned private-key detector spans whose interior is compiler read-only data."""

import base64
import copy
import subprocess
import tarfile
import unittest
from unittest.mock import patch

from image_secrets import elf_marker_gap, elf_nul_key_view
from image_secrets.binary_view import TEXT_BYTES
from image_secrets.common import digest_bytes
from image_secrets.gate import FORMAT_MARKER_ADVISORY, KEY_MARKER_VIEW_ADVISORY, KEY_MARKER_VIEW_RULE
from tests import fixtures
from tests import test_derived_payload as derived_tests
from tests import test_key_marker_view as key_tests
from tests import test_package_policy as packages
from tests.test_elf_nul_key_view import OFFSET, elf
from tests.test_pooled_literals import POOLED, pool

PATH = "usr/lib/libfixture-gap.so.1"
SOURCE = ("src/fixture-labels.c", "a" * 64)
# Ordinary compiler data: diagnostics, option names, a jump table and an OID.
INTERIOR = b"\0".join((
    b"\n", b"Client key is too short", b"Failure reading TLS data: %s", b"RequireUsername",
    b"\x30\xfe\xff\xff\x48\xfe\xff\xff\x60\xfe\xff\xff", b"*\x86H\x86\xf7\r\x01\x01\x01",
    b"N11QTlsPrivate10TlsKeyBaseE", b"Unrecognized command %1 = %2", b"",
))
TERMINATOR = b"-----END " + b"PRIVATE KEY-----"
PLANTED = base64.b64encode(b"public fixture text, not a key; " * 2)[:64]
BUILD = b"/usr/src/debug/fixture/fixture/"
BUILD_PATHS = BUILD + b"common/rfb/SSecurityVncAuthentication.cxx\0" + BUILD + b"common/rdr/BufferedInputStream.h\0"
NEEDLE = b"Proc-" + b"Type:"


class MarkerGapTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    detector_scan = packages.PackageTests.detector_scan
    manifest = packages.PackageTests.manifest
    publish = derived_tests.DerivedPayloadTests.publish
    refused = key_tests.KeyMarkerViewTests.refused

    def fixture(self, *, interior=INTERIOR, terminator=TERMINATOR, after=b"\0", extra=b""):
        body = b"\0" + key_tests.BEGIN + b"\0" + interior + terminator + after
        contents = elf(body + b"\0" * 8 + extra)
        supplied = fixtures.tar_bytes([(PATH, contents)])
        layer = fixtures.tar_bytes([(PATH, contents), ("etc/machine-id", b""),
                                    ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id"))])
        archive = self.root / "marker-gap.tar"
        fixtures.oci_archive(archive, [layer])
        raw = self.detector_scan(archive, "runtime-omarchy")
        [image] = raw["inventory"]["images"]
        start = OFFSET + 1
        stop = start + len(key_tests.BEGIN) + 1 + len(interior) + len(terminator)
        physical = contents[start:stop]
        detector = physical.translate(TEXT_BYTES)
        leak = next(row for row in raw["blocking_findings"] if row["rule"] == KEY_MARKER_VIEW_RULE
                    and row["candidate_sha256"] == digest_bytes(detector))
        manifest = self.manifest(supplied, layer, component="runtime-omarchy", file=PATH)
        manifest["packages"][0].update(name="fixture", version="1.0-1", authority_type="arch-signature",
                                       architecture="x86_64")
        [row] = manifest["entries"]
        package = manifest["packages"][0]
        match = {
            "original": elf_marker_gap.span(start, stop - start, digest_bytes(physical)),
            "detector_sha256": digest_bytes(detector),
            "begin": elf_marker_gap.label(start, len(key_tests.BEGIN), SOURCE, 1, digest_bytes(key_tests.BEGIN)),
            "terminator": elf_marker_gap.label(stop - len(terminator), len(terminator), SOURCE, 2,
                                               digest_bytes(terminator)),
            "interior_sha256": digest_bytes(contents[start + len(key_tests.BEGIN):stop - len(terminator)]),
        }
        pin = {"version": package["version"], "architecture": "x86_64", "artifact_sha256": package["artifact_sha256"],
               "file_sha256": row["file_sha256"], "matches": (match,)}
        identity = {"archive_sha256": raw["archive_sha256"], "image_config": image["config_digest"],
                    "image_manifest": image["manifest"]}
        row["elf_nul_key_view"] = {**identity, "encoding": elf_nul_key_view.ENCODING, "marker_gap": elf_marker_gap.MODE,
                                   "matches": [dict(elf_marker_gap.expected(match), finding_id=leak["finding_id"])]}
        marker = next(item for item in raw["blocking_findings"] if item["rule"] == "content.private-key-header")
        row["reviewed_literals"] = {**identity, "findings": [{
            "rule": marker["rule"], "finding_id": marker["finding_id"],
            "spans": [{"offset": start, "size": len(key_tests.BEGIN), "sha256": digest_bytes(key_tests.BEGIN),
                       "context": {"offset": start - 1, "size": len(key_tests.BEGIN) + 2,
                                   "sha256": digest_bytes(contents[start - 1:start + len(key_tests.BEGIN) + 1])},
                       "source": {"path": SOURCE[0], "line": 1, "sha256": SOURCE[1]}}]}]}
        row["matches"] = [
            {"rule": leak["rule"], "candidate_sha256": leak["candidate_sha256"], "line": None,
             "explanation": KEY_MARKER_VIEW_ADVISORY, "proof_sha256": "b" * 64},
            {"rule": marker["rule"], "candidate_sha256": None, "line": None,
             "explanation": FORMAT_MARKER_ADVISORY, "proof_sha256": "b" * 64},
        ]
        return archive, manifest, raw, pin

    def pinned(self, pin):
        return patch.multiple(elf_marker_gap, PINS={("fixture", PATH): pin}, BY_FILE={pin["file_sha256"]: pin})

    def test_pinned_interior_clears_only_the_reviewed_span(self):
        archive, manifest, raw, pin = self.fixture()
        self.assertIsNone(self.refused(archive, manifest, "unpinned", classification="content_refused"))
        with self.pinned(pin):
            clean = self.publish(archive, manifest, "marker-gap")
        self.assertEqual(clean["findings"], raw["findings"])
        self.assertEqual(clean["blocking_findings"], [])
        [row] = [item for item in clean["package_evidence"]["entries"] if item["classification"] == KEY_MARKER_VIEW_ADVISORY]
        self.assertEqual(row["elf_nul_key_view"]["marker_gap"], elf_marker_gap.MODE)

    def test_key_shaped_interiors_and_open_labels_stay_blocking(self):
        crlf = PLANTED[:32] + b"\r\n" + PLANTED[32:]
        # Each physical half is short; only the CR/LF-joined view is a full armor line.
        self.assertIsNone(elf_marker_gap.ARMOR_RUN.search(crlf))
        self.assertFalse(elf_marker_gap.public_interior(crlf))
        for name, options in (
            ("base64-line", {"interior": INTERIOR + PLANTED + b"\0"}),
            ("crlf-wrapped-base64", {"interior": INTERIOR + crlf + b"\0"}),
            ("encrypted-pem-header", {"interior": INTERIOR + b"Proc-" + b"Type: 4,ENCRYPTED\0"}),
            ("second-private-label", {"interior": b"\n\0-----BEGIN EC " + b"PRIVATE KEY-----\0" + INTERIOR[2:]}),
            ("unterminated-label", {"after": b"X"}),
        ):
            with self.subTest(name=name):
                archive, manifest, raw, pin = self.fixture(**options)
                with self.pinned(pin):
                    refused = self.publish(archive, manifest, name, blocks=True)
                self.assertEqual(refused["findings"], raw["findings"])
                self.assertIn(KEY_MARKER_VIEW_RULE, {item["rule"] for item in refused["blocking_findings"]})

    def test_changed_member_identity_or_interior_digest_refuses(self):
        archive, manifest, _, pin = self.fixture()
        item = manifest["entries"][0]["elf_nul_key_view"]["matches"][0]
        for name, change in {
            "path": lambda value: value["entries"][0].update(path=PATH + ".other"),
            "version": lambda value: value["packages"][0].update(version="1.0-2"),
            "file": lambda value: value["entries"][0].update(file_sha256="c" * 64),
            "interior": lambda value: value["entries"][0]["elf_nul_key_view"]["matches"][0]["interior"].update(
                sha256=digest_bytes(b"other")),
            "terminator": lambda value: value["entries"][0]["elf_nul_key_view"]["matches"][0]["declarations"][1].update(
                line=3),
            "extra-match": lambda value: value["entries"][0]["elf_nul_key_view"]["matches"].append(copy.deepcopy(item)),
        }.items():
            with self.subTest(name=name), self.pinned(pin):
                altered = copy.deepcopy(manifest)
                change(altered)
                self.assertIsNone(self.refused(archive, altered, name, classification="content_refused"))
        # Even with every outer digest re-pinned, a changed interior byte refuses at scan time.
        data = bytearray(elf(b"\0" + key_tests.BEGIN + b"\0" + INTERIOR + TERMINATOR + b"\0" + b"\0" * 8))
        match = pin["matches"][0]
        start, size = match["original"]["offset"], match["original"]["size"]
        self.assertEqual(digest_bytes(bytes(data[start:start + size])), match["original"]["sha256"])
        data[start + len(key_tests.BEGIN) + 5] ^= 1
        changed = bytes(data)
        physical = changed[start:start + size]
        moved = dict(match, original=dict(match["original"], sha256=digest_bytes(physical)),
                     detector_sha256=digest_bytes(physical.translate(TEXT_BYTES)))
        moved_pin = dict(pin, file_sha256=digest_bytes(changed), matches=(moved,))
        proof = {"matches": [elf_marker_gap.expected(moved)]}
        with patch.multiple(elf_marker_gap, BY_FILE={moved_pin["file_sha256"]: moved_pin}):
            self.assertEqual(elf_marker_gap.verify(changed, changed.translate(TEXT_BYTES), b"", proof), {})
            proof["matches"][0]["interior"]["sha256"] = digest_bytes(
                changed[start + len(key_tests.BEGIN):start + size - len(TERMINATOR)])
            moved["interior_sha256"] = proof["matches"][0]["interior"]["sha256"]
            self.assertEqual(set(elf_marker_gap.verify(changed, changed.translate(TEXT_BYTES), b"", proof)),
                             {moved["detector_sha256"]})

    def test_independent_key_material_elsewhere_keeps_the_file_blocking(self):
        generated = subprocess.run(["openssl", "genpkey", "-algorithm", "EC", "-pkeyopt",
                                    "ec_paramgen_curve:prime256v1", "-outform", "DER"],
                                   capture_output=True, check=True, timeout=10).stdout
        archive, manifest, raw, pin = self.fixture(extra=generated)
        with self.pinned(pin):
            refused = self.publish(archive, manifest, "independent-key", blocks=True)
        self.assertEqual(refused["findings"], raw["findings"])
        self.assertIn("content.binary-private-key", {row["rule"] for row in refused["blocking_findings"]})
        self.assertNotIn(KEY_MARKER_VIEW_ADVISORY,
                         {row["classification"] for row in refused["package_evidence"]["entries"]})

    def excepted(self, pin, **exception):
        return dict(pin, matches=(dict(pin["matches"][0], **exception),))

    def outcome(self, archive, manifest, pin, name):
        with self.pinned(pin):
            report = self.publish(archive, manifest, name, blocks=True)
        return {item["rule"] for item in report["blocking_findings"]}

    def test_pinned_build_paths_mask_only_their_exact_strings(self):
        archive, manifest, raw, pin = self.fixture(interior=INTERIOR + BUILD_PATHS)
        self.assertIn(KEY_MARKER_VIEW_RULE, self.outcome(archive, manifest, pin, "strict-paths"))
        with self.pinned(self.excepted(pin, build_paths={"prefix": BUILD, "count": 2})):
            clean = self.publish(archive, manifest, "build-paths")
        self.assertEqual(clean["findings"], raw["findings"])
        self.assertEqual(clean["blocking_findings"], [])
        for name, (interior, paths) in {
            "wrong-count": (INTERIOR + BUILD_PATHS, {"prefix": BUILD, "count": 3}),
            "other-prefix": (INTERIOR + BUILD_PATHS, {"prefix": b"/usr/src/debug/other/other/", "count": 2}),
            "armor-in-path": (INTERIOR + BUILD_PATHS + BUILD + b"A" * 40 + b"+B.cxx\0", {"prefix": BUILD, "count": 2}),
            "base64-beside-paths": (INTERIOR + BUILD_PATHS + PLANTED + b"\0", {"prefix": BUILD, "count": 2}),
        }.items():
            with self.subTest(name=name):
                archive, manifest, _, pin = self.fixture(interior=interior)
                self.assertIn(KEY_MARKER_VIEW_RULE,
                              self.outcome(archive, manifest, self.excepted(pin, build_paths=paths), name))

    def field(self, interior, offset_shift=0):
        archive, manifest, raw, pin = self.fixture(interior=interior)
        at = OFFSET + 1 + len(key_tests.BEGIN) + 1 + interior.index(NEEDLE) + offset_shift
        field = elf_marker_gap.label(at, len(NEEDLE), SOURCE, 3, digest_bytes(NEEDLE))
        return archive, manifest, raw, pin, field

    def test_pinned_field_name_masks_only_a_bare_needle(self):
        archive, manifest, raw, pin, field = self.field(INTERIOR + NEEDLE + b"\0")
        self.assertIn(KEY_MARKER_VIEW_RULE, self.outcome(archive, manifest, pin, "strict-field"))
        with self.pinned(self.excepted(pin, field_names=(field,))):
            clean = self.publish(archive, manifest, "field-name")
        self.assertEqual(clean["blocking_findings"], [])
        for name, (interior, shift) in {
            "field-with-value": (INTERIOR + NEEDLE + b" 4,ENCRYPTED\0", 0),
            "moved-field": (INTERIOR + NEEDLE + b"\0", 1),
            "second-field": (INTERIOR + NEEDLE + b"\0DEK-" + b"Info:\0", 0),
        }.items():
            with self.subTest(name=name):
                archive, manifest, _, pin, field = self.field(interior, shift)
                self.assertIn(KEY_MARKER_VIEW_RULE,
                              self.outcome(archive, manifest, self.excepted(pin, field_names=(field,)), name))

    def pooled(self, misalign=0):
        base = OFFSET + 1 + len(key_tests.BEGIN) + 1 + len(INTERIOR)
        interior = INTERIOR + b"\0" * ((-base) % 16 + misalign)
        archive, manifest, raw, pin = self.fixture(interior=interior, terminator=POOLED,
                                                   after=pool()[len(POOLED):] + b"\0" * 16)
        start = pin["matches"][0]["terminator"]["offset"]
        context = {"offset": start, "size": len(pool()), "sha256": digest_bytes(pool())}
        return archive, manifest, raw, pin, context

    def test_pinned_pool_may_terminate_only_its_exact_span(self):
        archive, manifest, raw, pin, context = self.pooled()
        self.assertIn(KEY_MARKER_VIEW_RULE, self.outcome(archive, manifest, pin, "strict-pool"))
        cleared = self.outcome(archive, manifest, self.excepted(pin, terminator_pool=context), "pooled-terminator")
        # The pooled label is also a second content marker, which this row leaves to its own proof.
        self.assertNotIn(KEY_MARKER_VIEW_RULE, cleared)
        for name, (options, change) in {
            "changed-pool": ({}, {"sha256": digest_bytes(b"other pool")}),
            "misaligned-pool": ({"misalign": 1}, {}),
        }.items():
            with self.subTest(name=name):
                archive, manifest, _, pin, context = self.pooled(**options)
                self.assertIn(KEY_MARKER_VIEW_RULE, self.outcome(
                    archive, manifest, self.excepted(pin, terminator_pool=dict(context, **change)), name))

    def test_reviewed_rows_pin_complete_label_extents(self):
        for (name, path), pin in elf_marker_gap.PINS.items():
            for match in pin["matches"]:
                with self.subTest(path=path, offset=match["original"]["offset"]):
                    reference = elf_marker_gap.expected(match)
                    begin, terminator = reference["declarations"]
                    original = reference["original"]
                    self.assertEqual(begin["offset"], original["offset"])
                    self.assertEqual(terminator["offset"] + terminator["size"], original["offset"] + original["size"])
                    self.assertGreater(reference["interior"]["size"], 0)
                    self.assertLessEqual(original["size"], elf_marker_gap.MAX_SPAN)
                    self.assertIn(name, {"tigervnc", "qt6-base"})


if __name__ == "__main__":
    unittest.main()
