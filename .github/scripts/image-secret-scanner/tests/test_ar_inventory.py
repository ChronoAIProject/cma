"""Reproduced GNU special-member bypasses and complete physical inventory."""

import gzip
import struct
import unittest

from image_secrets.ar_inventory import inventory
from image_secrets.common import ScanError
from tests import test_scanner as scanner_tests
from tests.fixtures import docker_archive, fake_key, tar_bytes
from tests.test_ar import ar_bytes


class ArInventoryTests(unittest.TestCase):
    setUp = scanner_tests.ScannerTests.setUp
    tearDown = scanner_tests.ScannerTests.tearDown
    scan = scanner_tests.ScannerTests.scan

    def image(self, contents):
        archive = self.root / "image.tar"
        docker_archive(archive, [tar_bytes([("usr/lib/review.a", contents)])])
        return archive

    def test_gnu_hidden_compressed_payload_probes_cannot_be_clean(self):
        for name, prefix in (("//", b""), ("/", bytes(4)), ("/SYM64/", bytes(8))):
            with self.subTest(member=name):
                contents = ar_bytes([(name, prefix + gzip.compress(fake_key()))])
                report, _ = self.scan(self.image(contents), 2, ["--blocking-rules=credential"])
                self.assertFalse(report["complete"])

    @staticmethod
    def gnu(width=4, *, target=None, name_offset=0):
        symbols = (1).to_bytes(width, "big") + bytes(width) + b"symbol\0"
        name = b"long_object_file_name.o/\n"
        member_offset = 8 + 60 + len(symbols) + len(symbols) % 2 + 60 + len(name) + len(name) % 2
        symbols = (
            symbols[:width]
            + (member_offset if target is None else target).to_bytes(width, "big")
            + symbols[2 * width :]
        )
        return ar_bytes(
            [
                ("/" if width == 4 else "/SYM64/", symbols),
                ("//", name),
                ("/" + str(name_offset), b"plain object"),
            ]
        )

    @staticmethod
    def bsd(*, target=None, name_offset=0):
        symbols = struct.pack("<I", 8) + bytes(8) + struct.pack("<I", 7) + b"symbol\0"
        offset = 8 + 60 + len(symbols) + len(symbols) % 2
        symbols = (
            struct.pack("<III", 8, name_offset, offset if target is None else target)
            + struct.pack("<I", 7)
            + b"symbol\0"
        )
        return ar_bytes([("__.SYMDEF", symbols), ("a_long_bsd_extended_member_name.o", b"plain object")])

    def test_valid_gnu_32_64_tables_and_bsd_extended_names(self):
        for label, data in (("gnu32", self.gnu()), ("gnu64", self.gnu(8)), ("bsd", self.bsd())):
            with self.subTest(format=label):
                report, _ = self.scan(self.image(data), 0)
                self.assertTrue(report["complete"])

    def test_bad_table_offsets_unconsumed_bytes_and_truncated_padding_refuse(self):
        cases = [
            self.gnu(target=9),
            self.gnu(name_offset=1),
            self.bsd(target=9),
            self.bsd(name_offset=1),
            ar_bytes([("//", b"unterminated")]),
            ar_bytes([("/", bytes(4) + b"extra")]),
            ar_bytes([("__.SYMDEF", bytes(8) + b"extra")]),
            ar_bytes([("file.o", b"odd")])[:-1],
            ar_bytes([("file.o", b"even")]) + b"x",
        ]
        for number, data in enumerate(cases):
            with self.subTest(case=number):
                report, _ = self.scan(self.image(data), 2)
                self.assertFalse(report["complete"])

    def test_physical_member_count_includes_filtered_metadata(self):
        path = self.root / "library.a"
        path.write_bytes(self.gnu())
        self.assertEqual(len(inventory(path, 3)), 3)
        with self.assertRaises(ScanError):
            inventory(path, 2)


if __name__ == "__main__":
    unittest.main()
