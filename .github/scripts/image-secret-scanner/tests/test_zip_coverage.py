"""Unlisted physical ZIP members cannot be mistaken for a harmless preamble."""

import gzip
import io
import os
import struct
import unittest
import zipfile

from image_secrets.common import ScanError
from image_secrets.containers import decode_nested
from tests import test_runtime_archives as runtime_tests


def omission_cases(secret):
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("hidden.gz", gzip.compress(secret))
        archive.writestr("public.txt", b"ordinary")
    original = output.getvalue()
    first = original.index(b"PK\x01\x02")
    length = 46 + sum(struct.unpack_from("<HHH", original, first + 28))
    # Retain every physical local record, changing only directory inventory.
    one = bytearray(original[:first] + original[first + length:])
    struct.pack_into("<HH", one, len(one) - 14, 1, 1)
    struct.pack_into("<L", one, len(one) - 10, len(one) - 22 - first)
    zero = bytearray(original[:first] + original[-22:])
    struct.pack_into("<HH", zero, len(zero) - 14, 0, 0)
    struct.pack_into("<L", zero, len(zero) - 10, 0)
    return original, bytes(one), bytes(zero)


def empty_zip():
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w"):
        pass
    return output.getvalue()


class ZipCoverageTests(unittest.TestCase):
    setUp = runtime_tests.RuntimeArchiveScanTests.setUp
    tearDown = runtime_tests.RuntimeArchiveScanTests.tearDown
    scan = runtime_tests.RuntimeArchiveScanTests.scan
    image = runtime_tests.RuntimeArchiveScanTests.image

    def test_unlisted_first_and_all_members_refuse_with_secret_detection_control(self):
        value = "ghp_" + os.urandom(18).hex()
        original, one, zero = omission_cases(("token=" + value + "\n").encode())
        report, public = self.scan(self.image(original), 1, ["--blocking-rules=credential"])
        self.assertTrue(any(row["rule"] == "gitleaks.github-pat" for row in report["findings"]))
        self.assertNotIn(value, public)
        for index, forged in enumerate((one, zero, b"public prefix" + one, b"public prefix" + zero)):
            with self.subTest(case=index):
                report, public = self.scan(self.image(forged), 2)
                self.assertFalse(report["complete"])
                self.assertEqual(report["error"], "nested-zip-unexplained-prefix")
                self.assertNotIn(value, public)

    def test_genuine_empty_and_bounded_text_prefix_controls_remain_clean(self):
        for prefix in (b"", b"public executable prefix", b"#!/bin/sh\n\t# ordinary\r\n", b"#" * 65536):
            report, _ = self.scan(self.image(prefix + empty_zip()), 0)
            self.assertTrue(report["complete"])

    def test_prefix_itself_is_scanned_under_a_distinct_identity(self):
        value = "ghp_" + os.urandom(18).hex()
        payload = ("token=" + value + "\n").encode() + empty_zip()
        report, public = self.scan(self.image(payload), 1, ["--blocking-rules=credential"])
        self.assertTrue(any(row["rule"] == "gitleaks.github-pat" and row.get("member") == "prefix"
                            for row in report["findings"]))
        self.assertNotIn(value, public)

    def test_opaque_oversized_and_partial_record_prefixes_refuse(self):
        for prefix in (b"binary\0prefix", b"MZ\x90\0", b"PK\x03\x04", b"#" * 65537):
            report, _ = self.scan(self.image(prefix + empty_zip()), 2)
            self.assertFalse(report["complete"])

    def test_prefix_metadata_consumes_entry_and_byte_budgets(self):
        physical = self.root / "prefixed.zip"
        prefix = b"public prefix"
        physical.write_bytes(prefix + empty_zip())
        for stage in (runtime_tests.RecordingStage(max_entries=0),
                      runtime_tests.RecordingStage(max_bytes=len(prefix) - 1)):
            with self.assertRaises(ScanError):
                decode_nested(stage, physical, {"path": "prefix.zip"}, 0, "zip")
        stage = runtime_tests.RecordingStage()
        decode_nested(stage, physical, {"path": "prefix.zip"}, 0, "zip")
        self.assertEqual((stage.budget.entries, stage.budget.bytes), (1, len(prefix)))
