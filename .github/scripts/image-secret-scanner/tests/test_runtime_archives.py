"""Contained archive decoding retains detection, privacy and hard boundaries."""

import bz2
import gzip
import io
import os
import tempfile
import unittest
import zipfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import tests.test_scanner as scanner_tests
from image_secrets.ar_decoder import decode_ar
from image_secrets.ar_framing import frames
from image_secrets.common import Budget, ScanError
from image_secrets.containers import child_location, identify
from tests.fixtures import digest, docker_archive, tar_bytes


def ar_member(name, payload, mode=b"100644"):
    if isinstance(name, str):
        name = name.encode()
    return (name.ljust(16) + b"0".ljust(12) + b"0".ljust(6) + b"0".ljust(6)
            + mode.ljust(8) + str(len(payload)).encode().ljust(10) + b"`\n" + payload
            + (b"\n" if len(payload) % 2 else b""))


def ar_bytes(entries):
    return b"!<arch>\n" + b"".join(ar_member(name, payload) for name, payload in entries)


def zip_bytes(payload):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("member.txt", payload)
    return buffer.getvalue()


class RecordingStage:
    def __init__(self, max_bytes=1024 * 1024, max_entries=100, timeout=5):
        self.budget = Budget(max_bytes, max_entries)
        self.backend = SimpleNamespace(timeout=timeout)
        self.members = []

    def add(self, data, _location):
        self.budget.reserve(len(data))

    def file(self, stream, size, location, _path, _depth):
        self.budget.reserve(size)
        data = bytearray()
        while block := stream.read(65536):
            data.extend(block)
        if len(data) != size:
            raise ScanError("truncated-layer-entry")
        self.members.append((location, bytes(data)))


class ArDecoderTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / "container.dat"
        self.location = {"path": "private/container.dat", "image": 0, "layer": 0, "entry": 0}

    def decode(self, payload, stage=None, limit=1024 * 1024):
        self.path.write_bytes(payload)
        stage = stage or RecordingStage()
        decode_ar(stage, self.path, self.location, 0, limit, child_location)
        return stage

    def test_empty_ordinary_and_duplicate_members(self):
        self.assertEqual(self.decode(ar_bytes([])).members, [])
        stage = self.decode(ar_bytes([("same/", b"one"), ("same/", b"two")]))
        self.assertEqual([data for _, data in stage.members], [b"one", b"two"])
        self.assertEqual([location["member"] for location, _ in stage.members], ["0", "1"])
        self.assertEqual(stage.budget.entries, 2)
        stage = self.decode(b"!<arch>\n" + ar_member("gnu.o/", b"data", b"644"))
        self.assertEqual(stage.members[0][1], b"data")

    def test_gnu_metadata_and_long_names_are_scanned_as_separate_members(self):
        name = b"long-synthetic-object-file.o"
        table = name + b"/\n"
        payload = (b"!<arch>\n" + ar_member("/", bytes(4), b"")
                   + ar_member("//", table, b"") + ar_member("/0", b"contents"))
        stage = self.decode(payload)
        self.assertEqual([data for _, data in stage.members], [bytes(4), table, b"contents"])
        self.assertEqual(stage.members[2][0]["path"], "private/container.dat!" + name.decode())
        self.assertEqual(stage.budget.entries, 3)

    def test_bsd_long_names_preserve_content_and_identity(self):
        name = b"another-long-synthetic-object.o"
        stage = self.decode(ar_bytes([("#1/" + str(len(name)), name + b"data")]))
        self.assertEqual(stage.members[0][1], b"data")
        self.assertTrue(stage.members[0][0]["path"].endswith("!" + name.decode()))

    def test_physical_framing_refuses_truncation_padding_modes_and_trailing_bytes(self):
        valid = ar_bytes([("sample/", b"odd")])
        variants = [valid[:-1], valid + b"x", valid[:20], valid.replace(b"`\n", b"xx", 1),
                    valid[:-1] + b"x", ar_member("sample/", b"odd"),
                    b"!<arch>\n" + ar_member("link/", b"outside", b"120777"),
                    b"!<arch>\n" + ar_member("#1/99", b"short"),
                    valid[:56] + b"-1        " + valid[66:]]
        for payload in variants:
            self.path.write_bytes(payload)
            with self.subTest(case=variants.index(payload)), self.assertRaises(ScanError):
                frames(self.path, 1024 * 1024, 1024 * 1024, 100)

    def test_exact_member_entry_and_byte_limits(self):
        payload = ar_bytes([("one/", b"four")])
        self.path.write_bytes(payload)
        self.assertEqual(len(frames(self.path, 4, 4, 1)), 1)
        for limits in ((3, 4, 1), (4, 3, 1), (4, 4, 0)):
            with self.assertRaises(ScanError):
                frames(self.path, *limits)
        with self.assertRaises(ScanError):
            self.decode(payload, limit=3)
        with self.assertRaises(ScanError):
            self.decode(payload, RecordingStage(max_entries=0))
        with self.assertRaises(ScanError):
            self.decode(payload, RecordingStage(max_bytes=3))

    def test_thin_and_other_formats_refuse_before_native_decoding(self):
        for payload in (b"!<thin>\n", bz2.compress(b"ordinary data"), b"7z\xbc\xaf\x27\x1c", b"Rar!\x1a\x07"):
            self.path.write_bytes(payload)
            with self.assertRaises(ScanError):
                identify(self.path)

    def test_unsafe_names_and_unresolved_gnu_offsets_refuse(self):
        for name in ("../outside/", "/outside", "/999"):
            with self.assertRaises(ScanError):
                self.decode(ar_bytes([(name, b"data")]))

    def test_hard_deadline_and_unavailable_worker_refuse(self):
        with self.assertRaisesRegex(ScanError, "nested-ar-timeout"):
            self.decode(ar_bytes([("one/", b"data")]), RecordingStage(timeout=0))
        with patch("image_secrets.ar_decoder.subprocess.Popen", side_effect=OSError("private detail")):
            with self.assertRaisesRegex(ScanError, "ar-decoder-unavailable"):
                self.decode(ar_bytes([]))

    def test_worker_does_not_receive_credentials_or_python_preloads(self):
        from image_secrets import ar_decoder

        real = ar_decoder.subprocess.Popen
        seen = []

        def launch(*args, **kwargs):
            seen.append(kwargs["env"])
            return real(*args, **kwargs)

        with patch.dict(os.environ, {"GITHUB_TOKEN": "synthetic", "PYTHONPATH": "/not-imported",
                                    "LD_PRELOAD": "/not-loaded"}), patch.object(ar_decoder.subprocess, "Popen", launch):
            self.decode(ar_bytes([]))
        self.assertTrue(seen)
        self.assertTrue(all(not ({"GITHUB_TOKEN", "PYTHONPATH", "LD_PRELOAD"} & env.keys()) for env in seen))


class RuntimeArchiveScanTests(unittest.TestCase):
    setUp = scanner_tests.ScannerTests.setUp
    tearDown = scanner_tests.ScannerTests.tearDown
    scan = scanner_tests.ScannerTests.scan

    def image(self, data):
        archive = self.root / "input.tar"
        docker_archive(archive, [tar_bytes([("library.dat", data)])])
        return archive

    def test_secret_in_ar_gzip_and_gzip_inside_ar_blocks_with_member_identity(self):
        value = "ghp_" + os.urandom(18).hex()
        plaintext = ("token=" + value + "\n").encode()
        cases = [(ar_bytes([("settings/", plaintext)]), "0"), (gzip.compress(plaintext), "0"),
                 (ar_bytes([("binary.o/", b"\x7fELF\x00" + plaintext)]), "0"),
                 (ar_bytes([("settings/", gzip.compress(plaintext))]), "0/0")]
        for payload, member in cases:
            report, output = self.scan(self.image(payload), 1, ["--blocking-rules=credential"])
            matches = [row for row in report["findings"] if row["rule"] == "gitleaks.github-pat"
                       and row.get("member") == member]
            self.assertTrue(matches)
            self.assertTrue(all(row["path"].startswith("path-sha256:") for row in matches))
            self.assertNotIn(value, output)

    def test_zip_bomb_member_limit_and_gzip_expansion_limit_refuse(self):
        plain = b"x" * 4096
        archive = self.image(zip_bytes(plain))
        with patch("image_secrets.engine.MAX_NESTED_MEMBER_BYTES", 1024):
            report, _ = self.scan(archive, 2)
        self.assertEqual(report["error"], "nested-member-size-limit")
        archive = self.image(gzip.compress(plain))
        with patch("image_secrets.containers.MAX_NESTED_MEMBER_BYTES", 1024):
            report, _ = self.scan(archive, 2)
        self.assertFalse(report["complete"])

    def test_deep_ar_and_compression_nesting_refuse(self):
        payload = b"ordinary content"
        for _ in range(4):
            payload = ar_bytes([("member/", payload)])
        report, _ = self.scan(self.image(payload), 2)
        self.assertEqual(report["error"], "nested-archive-depth-limit")
        payload = b"ordinary content"
        for _ in range(4):
            payload = gzip.compress(payload)
        report, _ = self.scan(self.image(payload), 2)
        self.assertEqual(report["error"], "nested-archive-depth-limit")

    def test_leading_elf_is_raw_scanned_even_with_embedded_zip_markers(self):
        payload = b"\x7fELF" + b"synthetic-binary" + zip_bytes(b"ordinary text")
        report, _ = self.scan(self.image(payload), 0)
        self.assertTrue(report["complete"])
        self.assertFalse(any("member" in row for row in report["findings"]))
        value = "ghp_" + os.urandom(18).hex()
        payload += ("\ntoken=" + value + "\n").encode()
        report, output = self.scan(self.image(payload), 1, ["--blocking-rules=credential"])
        self.assertIn("gitleaks.github-pat", {row["rule"] for row in report["findings"]})
        matches = [row for row in report["findings"] if row["rule"] == "gitleaks.github-pat"]
        self.assertTrue(all(row["file_sha256"] == digest(payload) for row in matches))
        self.assertTrue(all("line" not in row and "_file_sha256" not in row for row in matches))
        self.assertNotIn(value, output)

    def test_duplicate_ar_names_keep_distinct_exact_content_identities(self):
        value = "ghp_" + os.urandom(18).hex()
        secret = ("token=" + value + "\n").encode()
        payload = ar_bytes([("same/", secret), ("same/", b"replacement")])
        report, _ = self.scan(self.image(payload), 1, ["--blocking-rules=credential"])
        match = next(row for row in report["findings"] if row.get("member") == "0"
                     and row["rule"] == "gitleaks.github-pat")
        self.assertEqual(match["file_sha256"], digest(secret))
        self.assertEqual(match["path"], "path-sha256:" + digest(b"library.dat!same"))


if __name__ == "__main__":
    unittest.main()
