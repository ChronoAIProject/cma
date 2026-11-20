"""Check the exact NyxID comparison sequence at publication."""

import struct
import unittest

from image_secrets import nyxid_format_labels
from image_secrets.common import digest_bytes
from image_secrets.gate import FORMAT_MARKER_ADVISORY
from tests import test_derived_payload as derived
from tests import test_package_policy as packages

BASE = 0x400000
LABELS = (b"-----BEGIN OPENSSH " + b"PRIVATE KEY-----", b"-----BEGIN ENCRYPTED " + b"PRIVATE KEY-----")
LABEL_POSITIONS = (0x220, 0x220 + len(LABELS[0]))
CALLS = (0x320, 0x350)
SLOT = 0x4C0


def executable(*, wrong_import=False, wrong_length=False, clobber=False):
    data = bytearray(0x680)
    data[:7] = b"\x7fELF\x02\x01\x01"
    struct.pack_into("<HHIQQQIHHHHHH", data, 16, 3, 62, 1, BASE + CALLS[0], 64, 0, 0, 64, 56, 4, 0, 0, 0)
    segments = [(1, 4, 0, 0x300), (1, 5, 0x300, 0x100), (1, 6, 0x400, 0x280), (2, 6, 0x500, 128)]
    for i, (kind, flags, off, size) in enumerate(segments):
        struct.pack_into("<IIQQQQQQ", data, 64 + 56 * i, kind, flags, off, BASE + off, BASE + off, size, size, 1)
    for label, start, call in zip(LABELS, LABEL_POSITIONS, CALLS):
        data[start:start + len(label)] = label
        length = len(label) - int(wrong_length)
        first = b"\x4c\x89\xfe" if clobber else b"\x4c\x89\xff"
        code = (b"\xba" + struct.pack("<I", length) + first + b"\x48\x8d\x35" + struct.pack("<i", start - call - 15)
                + b"\xff\x15" + struct.pack("<i", SLOT - call - 21))
        data[call:call + len(code)] = code
    symbol = b"other" if wrong_import else b"bcmp"
    data[0x490:0x492 + len(symbol)] = b"\0" + symbol + b"\0"
    struct.pack_into("<QQq", data, 0x420, BASE + SLOT, 1 << 32 | 6, 0)
    struct.pack_into("<IBBHQQ", data, 0x450 + 24, 1, 0x12, 0, 0, 0, 0)
    dynamic = ((5, BASE + 0x490), (6, BASE + 0x450), (7, BASE + 0x420), (8, 24),
               (9, 24), (10, len(symbol) + 2), (11, 24), (0, 0))
    for i, (key, value) in enumerate(dynamic):
        struct.pack_into("<qQ", data, 0x500 + 16 * i, key, value)
    return bytes(data)


class NyxidFormatLabelsTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    manifest = packages.PackageTests.manifest
    detector_scan = packages.PackageTests.detector_scan
    fixture = derived.DerivedPayloadTests.fixture
    publish = derived.DerivedPayloadTests.publish

    def evidence(self, name, *, scope="runtime", **changes):
        data = executable(**changes)
        archive, packet, raw, _ = self.fixture(name, nyxid_format_labels.PATH, ("nyxid", data), data, component=scope)
        [row] = packet["entries"]
        row["path"] = nyxid_format_labels.PATH
        packet["packages"][0].update(name="nyxid-cli", version="0.10.0", authority_type="pinned-upstream")
        [finding] = [f for f in raw["blocking_findings"] if f["rule"] == nyxid_format_labels.RULE]
        [image] = raw["inventory"]["images"]

        def extent(start, size):
            return {"offset": start, "size": size, "sha256": digest_bytes(data[start:start + size])}

        comparisons = [{"label": extent(start, len(label)), "code": extent(call, nyxid_format_labels.COMPARE_BYTES)}
                       for label, start, call in zip(LABELS, LABEL_POSITIONS, CALLS)]
        row["nyxid_format_labels"] = {"source_commit": "1" * 40, "cargo_lock_sha256": "2" * 64,
                                     "crate_sha256": "3" * 64, "comparisons": comparisons}
        spans = [{**pair["label"], "context": extent(0x200, 0x100),
                  "source": {"path": "fixtures/keys.rs", "line": line, "sha256": "4" * 64}}
                 for line, pair in enumerate(comparisons, 1)]
        row["reviewed_literals"] = {"archive_sha256": raw["archive_sha256"], "image_config": image["config_digest"],
                                    "image_manifest": image["manifest"], "findings": [{"rule": finding["rule"],
                                                                                     "finding_id": finding["finding_id"],
                                                                                     "spans": spans}]}
        row["matches"] = [{"rule": finding["rule"], "line": None, "candidate_sha256": None,
                           "explanation": FORMAT_MARKER_ADVISORY, "proof_sha256": "5" * 64}]
        return archive, packet, raw, finding

    def test_both_exact_comparisons_yield_one_receipt_in_each_runtime(self):
        for scope in ("runtime", "runtime-omarchy"):
            with self.subTest(scope=scope):
                archive, packet, raw, finding = self.evidence(scope, scope=scope)
                siblings = [row for row in raw["blocking_findings"] if row["finding_id"] != finding["finding_id"]]
                result = self.publish(archive, packet, f"confirmed-{scope}", blocks=bool(siblings))
                self.assertEqual(result["findings"], raw["findings"])
                self.assertEqual(result["blocking_findings"], siblings)
                [receipt] = result["package_evidence"]["entries"]
                self.assertEqual(receipt["finding_id"], finding["finding_id"])

    def test_changed_call_or_length_keeps_the_raw_finding(self):
        for option in ("wrong_import", "wrong_length", "clobber"):
            with self.subTest(option=option):
                archive, packet, raw, _ = self.evidence(option, **{option: True})
                result = self.publish(archive, packet, f"refused-{option}", blocks=True)
                self.assertEqual(result["package_evidence"]["entries"], [])
                self.assertEqual(result["blocking_findings"], raw["blocking_findings"])
