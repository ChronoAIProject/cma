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
            "password-only-redis": b"\x00redis://:qa-fixture@localhost/0\x00",
            "nul-padded": b"\x00" + url + b"\x00",
            "unicode-user": b"\xff" + "https://δοκιμή:qa@example.invalid".encode() + b"\x00",
            "unicode-password": "https://qa:測試@example.invalid".encode(),
            "unicode-host": b"\x80" + "https://qa:xx@例え.invalid".encode() + b"\xff",
            "repeated-delimiter": b"https://qa:xx@@example.invalid",
            "non-special-backslash": rb"custom://qa:p\x@example.invalid",
            "normalized-userinfo": b"https://qa:<zz>@example.invalid",
            "percent-in-password": b"https://qa:q%zz@example.invalid",
            "unicode-all": "https://利用者:試験@例え.invalid".encode(),
            "split-marker": b"\x00" * (boundary - 7) + url,
            "long-userinfo": b"\x00" * (boundary - 10000) + b"git+ssh://qa:" + b"x" * 10010 + b"@[::1]/",
        }
        # The original zero-prefixed controls also look like empty tar archives.
        # Exercise their raw bytes and the strict tar refusal independently.
        for name in ("split-marker", "long-userinfo"):
            data = positives[name]
            self.assertTrue(credential_url.search(data))
            ambiguous = self.root / "ambiguous.tar"
            oci_archive(ambiguous, [tar_bytes([(name, data)])])
            refused, _ = self.scan(ambiguous, 2, ["--blocking-rules=credential"])
            self.assertFalse(refused["complete"])
            self.assertEqual(refused["error"], "tar-nonzero-trailer")
            # Keep the boundary offset identical in an unambiguous binary member.
            positives[name] = b"X" + data[1:]
        negatives = {
            "separate-strings": b"DebugLabelhttp://127.0.0.1:9000" + b"\x00" * 35 + b"DebugField@\xff",
            "web-backslash-path": rb"https://qa:p\x@example.invalid",
            "empty-password-redis": b"redis://:@localhost/0",
            "split-password-only": b"redis://:qa\x00fixture@localhost/0",
            "no-host": b"https://qa:xx@\x00example.invalid",
            "non-ascii": b"https://qa:xx\x80@example.invalid",
        }
        for index, invalid in enumerate((b"\xc0\x80", b"\xe2\x82", b"\xed\xa0\x80", b"\xf5\x80\x80\x80",
                                         "\u0085".encode(), "\u200b".encode())):
            negatives[f"user-boundary-{index}"] = b"https://u:" + invalid + b"p@example.invalid"
            negatives[f"host-boundary-{index}"] = b"https://u:p@" + invalid + b"example.invalid"
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
                          b"qa:xx@example%XZ", b"u:p#suffix@host", b"u:x/p@host", b"u:x?y@host", b"u:@host")):
            with self.subTest(case=index):
                self.assertFalse(credential_url.search(b"https://" + authority))

    def test_review_ranges_share_detection_and_include_explicit_port(self):
        first = b"https://" + b"fixture:example@sample.invalid:443"
        second = b"https://" + b"fixture:example@@sample.invalid"
        data = b"\0" + first + b"/path\0" + second + b"\0"
        offset = data.index(second)
        self.assertTrue(credential_url.search(data))
        self.assertEqual(list(credential_url.authorities(data)), [
            (1, 1 + len(first), False), (offset, offset + len(second), True),
        ])

    def test_review_authority_boundaries_keep_raw_detection(self):
        port = b"https://" + b"fixture:example@sample.invalid:443"
        host = b"https://" + b"fixture:example@[2001:db8::1]"
        for prefix, authority, suffix, ambiguous in (
            (b"", port, b"", False),
            (b"", port, b"?query", False),
            (b"", port, b"#fragment", False),
            (b"", port, b"\0", False),
            (b'"', port, b'"', False),
            (b"", port, b'"extra', True),
            (b"", port, b"@next.invalid", True),
            (b"", host, b"letters/path", True),
        ):
            with self.subTest(prefix=prefix, suffix=suffix):
                data = prefix + authority + suffix
                self.assertTrue(credential_url.search(data))
                self.assertEqual(list(credential_url.authorities(data)), [
                    (len(prefix), len(prefix) + len(authority), ambiguous),
                ])
