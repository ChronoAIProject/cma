"""Static-library decoding must not hide later, compressed or malformed members."""

import gzip
import unittest
from unittest.mock import patch

from image_secrets.ar_reader import members, run_ar
from image_secrets.common import Budget, ScanError
from tests import test_scanner as scanner_tests
from tests.fixtures import docker_archive, fake_key, tar_bytes
from tests.test_review_regressions import token


def ar_bytes(entries):
    data = bytearray(b"!<arch>\n")
    for name, value in entries:
        encoded = name.encode()
        # BSD extended names are decoded by both GNU and BSD archive readers.
        header_name = encoded
        payload = value
        if len(header_name) > 16:
            header_name = b"#1/" + str(len(encoded)).encode()
            payload = encoded + value
        header = (
            header_name.ljust(16)
            + b"0".ljust(12)
            + b"0".ljust(6)
            + b"0".ljust(6)
            + b"100644".ljust(8)
            + str(len(payload)).encode().ljust(10)
            + b"`\n"
        )
        data += header + payload
        if len(payload) % 2:
            data += b"\n"
    return bytes(data)


class ArTests(unittest.TestCase):
    setUp = scanner_tests.ScannerTests.setUp
    tearDown = scanner_tests.ScannerTests.tearDown
    scan = scanner_tests.ScannerTests.scan

    def image(self, data):
        archive = self.root / "image.tar"
        docker_archive(archive, [tar_bytes([("usr/lib/library.a", data)])])
        return archive

    def test_clean_empty_and_long_name_archives_complete(self):
        for entries in (
            [],
            [("one.o", b"plain object")],
            [("long_member_name_for_static_library.o", b"plain")],
        ):
            with self.subTest(entries=len(entries)):
                report, _ = self.scan(self.image(ar_bytes(entries)), 0)
                self.assertTrue(report["complete"])

    def test_later_member_and_nested_compression_do_not_hide_secrets(self):
        for secret in (fake_key(), token().encode()):
            data = ar_bytes([("first.o", b"benign"), ("later.o", gzip.compress(secret))])
            report, output = self.scan(self.image(data), 1, ["--blocking-rules=credential"])
            self.assertTrue(report["complete"])
            self.assertTrue(report["blocking_findings"])
            self.assertNotIn(secret.decode(), output)
            self.assertTrue(any("member" in finding for finding in report["findings"]))

    def test_duplicate_members_scan_every_occurrence_in_physical_order(self):
        clean = ar_bytes([("same.o", b"first clean object"), ("same.o", b"second clean object")])
        report, _ = self.scan(self.image(clean), 0)
        self.assertTrue(report["complete"])
        data = ar_bytes([("same.o", b"clean"), ("same.o", gzip.compress(fake_key()))])
        report, _ = self.scan(self.image(data), 1)
        self.assertTrue(report["complete"])
        self.assertTrue(any(f.get("member") == "1/0" for f in report["findings"]))

    def test_native_reader_omitting_duplicate_bytes_fails_closed(self):
        def incomplete(operation, source, destination, limit, timeout, member=None):
            run_ar(operation, source, destination, limit, timeout, member)
            if operation == "p":
                destination.write_bytes(destination.read_bytes()[:-1])

        with patch("image_secrets.ar_reader.run_ar", side_effect=incomplete):
            data = ar_bytes([("same.o", b"first"), ("same.o", b"second")])
            report, _ = self.scan(self.image(data), 2)
        self.assertEqual(report["error"], "ar-member-reader-disagrees")

    def test_thin_truncated_and_unreadable_archives_refuse(self):
        valid = ar_bytes([("member.o", b"payload")])
        for data in (b"!<thin>\n", b"!<arch>\ninvalid", valid[:-5]):
            with self.subTest(length=len(data)):
                report, _ = self.scan(self.image(data), 2)
                self.assertFalse(report["complete"])

    def test_unsafe_member_names_refuse(self):
        for name in ("../escape", "/absolute", "-option", "line\nbreak"):
            with self.subTest(name=name):
                report, output = self.scan(self.image(ar_bytes([(name, b"clean")])), 2)
                self.assertFalse(report["complete"])
                self.assertNotIn("escape", output)

    def test_nested_depth_limit_remains_a_refusal(self):
        data = b"plain"
        for _ in range(5):
            data = ar_bytes([("nested.o", data)])
        report, _ = self.scan(self.image(data), 2)
        self.assertFalse(report["complete"])

    def test_member_listing_and_output_limits(self):
        archive = self.root / "library.a"
        archive.write_bytes(ar_bytes([("one.o", b"a" * 100), ("two.o", b"b")]))
        with self.assertRaises(ScanError):
            list(members(archive, self.root, Budget(1000, 1), 10))
        with self.assertRaises(ScanError):
            run_ar("p", archive, self.root / "member", 10, 10, "one.o")
        self.assertFalse((self.root / "ar-members.txt").exists())

    def test_missing_reader_fails_closed(self):
        with patch("image_secrets.ar_reader.subprocess.Popen", side_effect=FileNotFoundError):
            report, _ = self.scan(self.image(ar_bytes([("a.o", b"clean")])), 2)
        self.assertFalse(report["complete"])


if __name__ == "__main__":
    unittest.main()
