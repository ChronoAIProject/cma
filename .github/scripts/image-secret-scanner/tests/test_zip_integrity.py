"""Full CLI regressions: ZIP declarations never cap secret detection silently."""

import io
import os
import struct
import unittest
import warnings
import zipfile
import zlib
from unittest.mock import patch

from tests import test_runtime_archives as runtime_tests
from tests.test_runtime_archives import zip_bytes


def altered_zip(data, size=None, crc=None, packed=None):
    data = bytearray(data)
    central = data.index(b"PK\x01\x02")
    start = 30 + sum(struct.unpack_from("<HH", data, 26))
    if packed is not None:
        data[start:central] = packed
        central = start + len(packed)
        struct.pack_into("<L", data, 18, len(packed))
        struct.pack_into("<L", data, central + 20, len(packed))
        struct.pack_into("<L", data, len(data) - 6, central)
    for value, local, relative in ((size, 22, 24), (crc, 14, 16)):
        if value is not None:
            struct.pack_into("<L", data, local, value)
            struct.pack_into("<L", data, central + relative, value)
    return bytes(data)


class Nonseekable(io.BytesIO):
    def seek(self, *args):
        raise OSError("fixture-nonseekable")


class ZipIntegrityTests(unittest.TestCase):
    setUp = runtime_tests.RuntimeArchiveScanTests.setUp
    tearDown = runtime_tests.RuntimeArchiveScanTests.tearDown
    scan = runtime_tests.RuntimeArchiveScanTests.scan
    image = runtime_tests.RuntimeArchiveScanTests.image

    def test_forged_prefix_size_and_crc_cannot_hide_a_secret(self):
        value = "ghp_" + os.urandom(18).hex()
        packed = zip_bytes(b"A\ntoken=" + value.encode() + b"\n")
        report, output = self.scan(self.image(altered_zip(packed, size=1, crc=zlib.crc32(b"A"))), 2)
        self.assertFalse(report["complete"])
        self.assertNotIn(value, output)

    def test_underreported_overreported_sizes_and_crc_refuse(self):
        payload = b"ordinary data"
        original = zip_bytes(payload)
        for changed in (altered_zip(original, size=0), altered_zip(original, size=len(payload) + 1),
                        altered_zip(original, crc=zlib.crc32(payload[:1]))):
            report, _ = self.scan(self.image(changed), 2)
            self.assertFalse(report["complete"])

    def test_trailing_compressed_data_truncation_and_local_disagreement_refuse(self):
        original = zip_bytes(b"ordinary data")
        central = original.index(b"PK\x01\x02")
        start = 30 + sum(struct.unpack_from("<HH", original, 26))
        compressed = original[start:central]
        local_lie = bytearray(original)
        struct.pack_into("<L", local_lie, 22, 1)
        cases = [altered_zip(original, packed=compressed + b"trailer"),
                 altered_zip(original, packed=compressed + compressed),
                 altered_zip(original, packed=compressed[:-1]), bytes(local_lie)]
        for data in cases:
            report, _ = self.scan(self.image(data), 2)
            self.assertFalse(report["complete"])

    def test_ordinary_and_zip64_stored_deflate_and_descriptors_scan_all_content(self):
        value = "ghp_" + os.urandom(18).hex()
        for method in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED):
            for zip64 in (False, True):
                for descriptor in (False, True):
                    with self.subTest(method=method, zip64=zip64, descriptor=descriptor):
                        output = Nonseekable() if descriptor else io.BytesIO()
                        with patch("zipfile.ZIP64_LIMIT", 1 if zip64 else 2**31 - 1):
                            with zipfile.ZipFile(output, "w", compression=method) as archive:
                                archive.writestr("member.txt", ("token=" + value + "\n").encode())
                        report, public = self.scan(self.image(output.getvalue()), 1,
                                                   ["--blocking-rules=credential"])
                        self.assertTrue(report["complete"])
                        self.assertTrue(any(row.get("member") == "0" and row["rule"] == "gitleaks.github-pat"
                                            for row in report["findings"]))
                        self.assertNotIn(value, public)

    def test_clean_empty_members_and_directories(self):
        output = io.BytesIO()
        with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            archive.writestr("empty.txt", b"")
            archive.writestr("folder/", b"")
            archive.writestr("ordinary.txt", b"ordinary")
        report, _ = self.scan(self.image(output.getvalue()), 0)
        self.assertTrue(report["complete"])

    def test_streaming_zip64_zero_local_placeholders_remain_supported(self):
        # Python 3.9's nonseekable ZIP64 writer uses zero local sizes, an
        # extra field and a 64-bit descriptor, without local size sentinels.
        output = Nonseekable()
        with patch("zipfile.ZIP64_LIMIT", 1):
            with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
                archive.writestr("member", b"ordinary")
        data = bytearray(output.getvalue())
        struct.pack_into("<H", data, 4, 20)
        struct.pack_into("<LL", data, 18, 0, 0)
        report, _ = self.scan(self.image(bytes(data)), 0)
        self.assertTrue(report["complete"])

    def test_large_valid_output_and_duplicate_names_retain_earlier_secret(self):
        value = "ghp_" + os.urandom(18).hex()
        output = io.BytesIO()
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", UserWarning)
            with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
                archive.writestr("same", b"ordinary\n" * 20000 + ("token=" + value + "\n").encode())
                archive.writestr("same", b"ordinary")
        report, public = self.scan(self.image(output.getvalue()), 1, ["--blocking-rules=credential"])
        self.assertTrue(report["complete"])
        self.assertTrue(any(row.get("member") == "0" and row["rule"] == "gitleaks.github-pat"
                            for row in report["findings"]))
        self.assertNotIn(value, public)

    def test_bad_descriptor_and_unexplained_gap_refuse(self):
        output = Nonseekable()
        with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            archive.writestr("member", b"ordinary")
        data = bytearray(output.getvalue())
        descriptor = data.index(b"PK\x07\x08")
        data[descriptor + 4] ^= 1
        report, _ = self.scan(self.image(bytes(data)), 2)
        self.assertFalse(report["complete"])
        original = zip_bytes(b"ordinary")
        central = original.index(b"PK\x01\x02")
        data = bytearray(original[:central] + b"unexplained" + original[central:])
        struct.pack_into("<L", data, len(data) - 6, central + len(b"unexplained"))
        report, _ = self.scan(self.image(bytes(data)), 2)
        self.assertFalse(report["complete"])

    def test_nonempty_directory_and_unsupported_codec_refuse(self):
        for method, name in ((zipfile.ZIP_DEFLATED, "folder/"), (zipfile.ZIP_BZIP2, "member")):
            output = io.BytesIO()
            with zipfile.ZipFile(output, "w", compression=method) as archive:
                archive.writestr(name, b"ordinary")
            report, _ = self.scan(self.image(output.getvalue()), 2)
            self.assertFalse(report["complete"])

    def test_expansion_budget_refuses_before_large_allocation(self):
        image = self.image(zip_bytes(b"x" * 4096))
        with patch("image_secrets.zip_stream.MAX_NESTED_MEMBER_BYTES", 1024):
            report, _ = self.scan(image, 2)
        self.assertEqual(report["error"], "nested-member-size-limit")
        # A declared small size must not authorize even the bounded true output.
        image = self.image(altered_zip(zip_bytes(b"x" * 4096), size=1))
        with patch("image_secrets.zip_stream.MAX_NESTED_MEMBER_BYTES", 1024):
            report, _ = self.scan(image, 2)
        self.assertFalse(report["complete"])
