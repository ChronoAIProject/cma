"""BSD physical names cannot hide bytes behind the native C-string boundary."""

import io
import os
import unittest
from unittest.mock import Mock, patch

from image_secrets.ar_worker import decode
from image_secrets.common import ScanError
from tests import test_runtime_archives as runtime_tests
from tests.fixtures import digest
from tests.test_runtime_archives import ar_bytes


def bsd_member(name, data, padding=0):
    prefix = name + bytes(padding)
    return "#1/" + str(len(prefix)), prefix + data


class BsdArIntegrityTests(unittest.TestCase):
    setUp = runtime_tests.RuntimeArchiveScanTests.setUp
    tearDown = runtime_tests.RuntimeArchiveScanTests.tearDown
    scan = runtime_tests.RuntimeArchiveScanTests.scan
    image = runtime_tests.RuntimeArchiveScanTests.image

    def test_nonzero_suffix_after_nul_cannot_be_clean(self):
        value = "ghp_" + os.urandom(18).hex()
        prefix = b"member.o\0token=" + value.encode() + bytes(16)
        payload = ar_bytes([("#1/" + str(len(prefix)), prefix + b"ordinary")])
        report, output = self.scan(self.image(payload), 2)
        self.assertFalse(report["complete"])
        self.assertNotIn(value, output)

    def test_credential_in_actual_bsd_name_is_detected(self):
        value = "ghp_" + os.urandom(18).hex()
        payload = ar_bytes([bsd_member(("token=" + value).encode(), b"ordinary", 8)])
        report, output = self.scan(self.image(payload), 1, ["--blocking-rules=credential"])
        self.assertTrue(any(row["rule"] == "gitleaks.github-pat" and row.get("member") == "0"
                            for row in report["findings"]))
        self.assertNotIn(value, output)

    def test_long_names_with_and_without_nul_padding_remain_clean(self):
        for padding in (0, 1, 8):
            with self.subTest(padding=padding):
                payload = ar_bytes([bsd_member(b"ordinary-long-object-file-name.o", b"data", padding)])
                report, _ = self.scan(self.image(payload), 0)
                self.assertTrue(report["complete"])

    def test_malformed_prefixes_refuse(self):
        for member in (("#1/0", b"data"), ("#1/99", b"short"), ("#1/4", bytes(4) + b"data"),
                       ("#1/x", b"data"), ("#1/5", b"a\0x\0\0data")):
            with self.subTest(case=member[0]):
                report, _ = self.scan(self.image(ar_bytes([member])), 2)
                self.assertFalse(report["complete"])

    def test_duplicate_bsd_names_preserve_earlier_secret_and_ordinal(self):
        value = "ghp_" + os.urandom(18).hex()
        secret = ("token=" + value + "\n").encode()
        payload = ar_bytes([bsd_member(b"same-long-object-file.o", secret, 4),
                            bsd_member(b"same-long-object-file.o", b"ordinary", 4)])
        report, output = self.scan(self.image(payload), 1, ["--blocking-rules=credential"])
        match = next(row for row in report["findings"] if row["rule"] == "gitleaks.github-pat"
                     and row.get("member") == "0")
        self.assertEqual(match["file_sha256"], digest(secret))
        self.assertNotIn(value, output)

    def test_native_name_must_equal_physical_prefix(self):
        path = self.root / "container"
        path.write_bytes(ar_bytes([bsd_member(b"physical-name.o", b"data", 4)]))
        reader = Mock()
        reader.header.return_value = ("different-name.o", 4)
        with patch("image_secrets.ar_worker.ArReader", return_value=reader):
            with self.assertRaises(ScanError):
                decode(path, 1024, 1024, 1, io.BytesIO())
        reader.close.assert_called_once()
