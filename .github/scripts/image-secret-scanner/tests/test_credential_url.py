"""Policy A recognizes complete URI authorities even in arbitrary binary data."""

import unittest

from image_secrets import credential_url
from image_secrets.common import digest_bytes
from tests import test_scanner
from tests.fixtures import oci_archive, tar_bytes


class CredentialUrlTests(unittest.TestCase):
    setUp = test_scanner.ScannerTests.setUp
    tearDown = test_scanner.ScannerTests.tearDown
    scan = test_scanner.ScannerTests.scan

    def test_credential_locations_in_a_binary_archive(self):
        authority = b"qa:xx@example.invalid"
        url = b"https://" + authority
        boundary = 1024 * 1024
        positives = {
            "embedded": b"\xff" + url + b"\x80",
            "nul-padded": b"\x00" + url + b"\x00",
            "split-marker": b"\x00" * (boundary - 7) + url,
            "long-userinfo": b"\x00" * (boundary - 10000) + b"git+ssh://qa:" + b"x" * 10010 + b"@[::1]/",
        }
        negatives = {
            "separate-strings": b"DebugLabelhttp://127.0.0.1:9000" + b"\x00" * 35 + b"DebugField@\xff",
            "no-host": b"https://qa:xx@\x00example.invalid",
            "non-ascii": b"https://qa:xx\x80@example.invalid",
        }
        for separator in (0, 1, 9, 10, 13, 31, 127):
            negatives[f"control-{separator}"] = b"https://qa:xx" + bytes([separator]) + b"@example.invalid"
        archive = self.root / "controls.tar"
        oci_archive(archive, [tar_bytes([*positives.items(), *negatives.items()])])
        report, _ = self.scan(archive, 1, ["--blocking-rules=credential"])
        locations = {row["path"] for row in report["blocking_findings"] if row["rule"] == "content.credential-url"}
        self.assertTrue(report["complete"])
        self.assertEqual(locations, {"path-sha256:" + digest_bytes(name.encode()) for name in positives})

    def test_uri_authority_grammar(self):
        for scheme in (b"http", b"postgresql", b"git+ssh", b"x-1.2"):
            for name in (b"localhost", b"127.0.0.1:443", b"exa%6Dple.invalid", b"[2001:db8::1]",
                         b"[::ffff:192.0.2.1]", b"[fe80::1%25eth0]", b"[v1.a:b]"):
                with self.subTest(scheme=scheme, name=name):
                    self.assertTrue(credential_url.search(scheme + b"://u%40x:p%2Fq:r@" + name + b"/path"))
        for index, authority in enumerate((b"qa:xx@", b"qa:xx@/path", b"qa:xx@\xff", b"qa:xx@[bad]", b"qa:xx@[::1",
                          b"qa:xx@example%XZ", b"u%:xx@host", b"u:x%AZ@host", b"u:x?y@host", b"u:@host")):
            with self.subTest(case=index):
                self.assertFalse(credential_url.search(b"https://" + authority))
