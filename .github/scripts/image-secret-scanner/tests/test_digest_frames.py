"""Exercise the staging digest-encoding evidence against unchanged raw detections."""

import copy
import json
import sys
import tarfile
import unittest
from pathlib import Path
from unittest.mock import patch

from image_secrets.common import ScanError, digest_bytes
from image_secrets.gate import DIGEST_FRAME_ADVISORY, DIGEST_FRAME_RULE
from tests import fixtures
from tests import test_library_vectors as vectors
from tests import test_package_policy as packages

PATH = "usr/lib/libexample.so.1"
# The unlisted digest arc 2.6 keeps this synthetic frame detectable, so the
# package proof remains the only path that can classify it.
PREFIX = bytes.fromhex("3020300c06082a864886f70d020605000410")
PUBLIC_DATA = bytes(range(16))
COMPONENT = "runtime-omarchy"


class DigestFrameTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    manifest = packages.PackageTests.manifest
    policy = staticmethod(packages.PackageTests.policy)
    classify = packages.PackageTests.classify
    detector_scan = packages.PackageTests.detector_scan

    def fixture(self, *, appended=b"", copied=False, public_data=PREFIX + PUBLIC_DATA, frame_ranges=None, pax_headers=None):
        payload = vectors.ELF + public_data + appended
        layer = fixtures.tar_bytes([(PATH, payload), ("etc/machine-id", b""),
                                    ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id"))],
                                   pax_headers=pax_headers)
        archive = self.root / "digest-image.tar"
        layers = [layer]
        if copied:
            layers.append(fixtures.tar_bytes([(PATH, payload), ("example", b"app")]))
        fixtures.docker_archive(archive, layers)
        manifest = self.manifest(fixtures.tar_bytes([(PATH, payload)]), layer, component=COMPONENT, file=PATH)
        row = manifest["entries"][0]
        row["matches"] = [{"rule": DIGEST_FRAME_RULE, "candidate_sha256": None, "line": None,
                           "explanation": DIGEST_FRAME_ADVISORY, "proof_sha256": "e" * 64}]
        row["digest_frames"] = {
            "release": {"name": "example", "version": "1.0", "archive_sha256": "c" * 64,
                        "authority_sha256": "d" * 64},
            "frames": [self.span(public_data, offset, size) for offset, size in (frame_ranges or [(0, len(public_data))])],
        }
        return archive, manifest

    @staticmethod
    def span(data, offset, size):
        frame = bytes(data[offset:offset + size])
        return {"offset": len(vectors.ELF) + offset, "size": size, "sha256": digest_bytes(frame),
                "parts": [{"size": len(part), "sha256": digest_bytes(part), "source_member": source,
                           "source_line": 1, "proof_sha256": digest_bytes(source.encode())}
                          for part, source in ((frame[:len(PREFIX)], "src/digest.c"),
                                               (frame[len(PREFIX):], "src/adjacent.c"))]}

    def test_exact_public_frame_is_rechecked_with_a_warm_detector_cache(self):
        archive, manifest = self.fixture()
        cache = self.root / "digest-cache.json"
        opts = ["--detector-cache-context", "d" * 64, "--detector-cache-output", str(cache)]
        raw = self.detector_scan(archive, COMPONENT, opts)
        warm = self.detector_scan(archive, COMPONENT, [*opts, "--detector-cache-input", str(cache)])
        self.assertEqual(warm["detector_cache"]["hits"], 1)
        self.assertEqual([finding["rule"] for finding in raw["blocking_findings"]], [DIGEST_FRAME_RULE])
        evidence = self.policy(manifest)
        section, final = self.classify(archive, raw, evidence, COMPONENT)
        warm_section, warm_final = self.classify(archive, warm, evidence, COMPONENT)
        self.assertEqual(section, warm_section)
        self.assertEqual(final["counts"], warm_final["counts"])
        self.assertEqual(final["blocking_findings"], [])
        self.assertEqual(len(section["entries"]), 1)
        retained = section["entries"][0]
        self.assertEqual(retained["classification"], DIGEST_FRAME_ADVISORY)
        self.assertEqual(retained["public_digest_frames"], manifest["entries"][0]["digest_frames"])
        other = self.detector_scan(archive)
        refused_section, refused = self.classify(archive, other, evidence)
        self.assertEqual(refused_section["entries"], [])
        self.assertEqual(refused["blocking_findings"], other["blocking_findings"])

    def test_unaccounted_or_changed_bytes_keep_the_finding_blocking(self):
        archive, manifest = self.fixture()
        raw = self.detector_scan(archive, COMPONENT)
        for field in ("segment", "boundary", "frame", "package", "layer"):
            with self.subTest(field=field):
                wrong = copy.deepcopy(manifest)
                row = wrong["entries"][0]
                frame = row["digest_frames"]["frames"][0]
                if field == "segment":
                    frame["parts"][1]["sha256"] = "b" * 64
                elif field == "boundary":
                    for part, shifted in zip(frame["parts"], (PREFIX + PUBLIC_DATA[:1], PUBLIC_DATA[1:])):
                        part.update(size=len(shifted), sha256=digest_bytes(shifted))
                elif field == "frame":
                    frame["offset"] += 1
                elif field == "package":
                    row["file_sha256"] = row["package_member"]["sha256"] = "b" * 64
                else:
                    row["diff_id"] = "sha256:" + "b" * 64
                section, final = self.classify(archive, raw, self.policy(wrong), COMPONENT)
                self.assertEqual(section["entries"], [])
                self.assertEqual(final["blocking_findings"], raw["blocking_findings"])
        manifest["entries"][0]["digest_frames"]["frames"][0]["parts"].pop()
        with self.assertRaises(ScanError):
            self.policy(manifest)

    def test_unproved_binary_key_and_application_copy_remain_blocking(self):
        der_key = vectors.LibraryVectorTests.generated_key("-outform", "DER")
        for extra, copied in ((b"\x00" + der_key, False), (b"", True)):
            with self.subTest(copied=copied):
                archive, manifest = self.fixture(appended=extra, copied=copied)
                raw = self.detector_scan(archive, COMPONENT)
                section, final = self.classify(archive, raw, self.policy(manifest), COMPONENT)
                self.assertEqual(len(section["entries"]), int(copied))
                self.assertEqual([f["layer"] for f in final["blocking_findings"] if f["rule"] == DIGEST_FRAME_RULE],
                                 [int(copied)])

    def test_overlapping_and_contained_frames_require_the_full_set(self):
        data = bytearray(84)
        ranges = [(0, 34), (24, 60), (44, 22)]
        for start, size in ranges:
            prefix = b"\x30" + bytes([size - 2]) + PREFIX[2:-1] + bytes([size - len(PREFIX)])
            data[start:start + len(prefix)] = prefix
        archive, manifest = self.fixture(public_data=bytes(data), frame_ranges=ranges)
        raw = self.detector_scan(archive, COMPONENT)
        expected = manifest["entries"][0]["digest_frames"]["frames"]
        section, final = self.classify(archive, raw, self.policy(manifest), COMPONENT)
        self.assertEqual(final["blocking_findings"], [])
        self.assertEqual(section["entries"][0]["public_digest_frames"]["frames"], expected)
        omitted = copy.deepcopy(manifest)
        omitted["entries"][0]["digest_frames"]["frames"].pop()
        section, final = self.classify(archive, raw, self.policy(omitted), COMPONENT)
        self.assertEqual(section["entries"], [])
        self.assertEqual([f["rule"] for f in final["blocking_findings"]], [DIGEST_FRAME_RULE])
        expected.insert(1, expected[0])
        with self.assertRaises(ScanError):
            self.policy(manifest)

    def test_publication_requires_the_reconstructed_frame_receipt(self):
        archive, manifest = self.fixture(pax_headers={PATH: fixtures.CAPABILITY_PAX})
        scripts = Path(__file__).resolve().parents[2]
        with patch.object(sys, "path", [str(scripts), *sys.path]):
            import image_secret_gate as gate

            manifest["scanner_manifest_sha256"] = gate.MANIFEST_SHA256
            raw = json.dumps(manifest).encode()
            output = self.root / "digest-publication"
            with patch.object(gate, "load_package_evidence", return_value=(raw, digest_bytes(raw), packages.REVIEW)):
                digest, _ = gate.verified_staging_scan(archive, output, component=COMPONENT)
            report = json.loads((output / "REPORT.json").read_bytes())
            expected = copy.deepcopy(report["package_evidence"])
            report["package_evidence"]["entries"][0]["public_digest_frames"]["frames"][0]["parts"].pop()
            with self.assertRaises(gate.ScanRefusal):
                gate.validate_receipt(report, digest, "credential", None, component=COMPONENT,
                                      package_section=expected)

    def test_receipt_reopens_binary_xattrs_before_accepting_a_frame(self):
        key = vectors.LibraryVectorTests.generated_key("-outform", "DER")
        xattr = {fixtures.CAPABILITY_XATTR: key.decode("utf-8", "surrogateescape")}
        archive, manifest = self.fixture(pax_headers={PATH: xattr})
        report = self.detector_scan(archive, COMPONENT)
        self.assertTrue(any(row["rule"] == DIGEST_FRAME_RULE and row.get("view") == "pax-xattr"
                            for row in report["findings"]))
        # The saved artifact must still prevent the decision if detector
        # metadata lacks the binary attribute observation.
        report["findings"] = [row for row in report["findings"] if row.get("view") != "pax-xattr"]
        section, assessed = self.classify(archive, report, self.policy(manifest), COMPONENT)
        self.assertEqual(section["entries"], [])
        self.assertIn(DIGEST_FRAME_RULE, {row["rule"] for row in assessed["blocking_findings"]})
