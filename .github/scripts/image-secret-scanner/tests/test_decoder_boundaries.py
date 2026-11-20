"""Decoder resource, native-error and protocol failures never yield CLEAN."""

import io
import json
import lzma
import struct
import subprocess
import unittest
from unittest.mock import Mock, patch

from image_secrets.ar_decoder import MemberStream, exact
from image_secrets.ar_native import ArReader
from image_secrets.ar_worker import decode, main, raw_chunks
from image_secrets.binary_view import prepare
from image_secrets.common import Budget, ScanError
from image_secrets.containers import decode_nested, inspect_nested
from image_secrets.xz_stream import decode_xz
from tests import test_runtime_archives as runtime_tests
from tests.fixtures import digest, tar_bytes
from tests.test_runtime_archives import RecordingStage, ar_bytes, ar_member, zip_bytes


class DecoderBoundaryTests(unittest.TestCase):
    setUp = runtime_tests.ArDecoderTests.setUp

    def test_worker_output_exact_protocol_and_metadata_payload(self):
        self.path.write_bytes(b"!<arch>\n" + ar_member("//", b"long-object-name.o/\n", b"")
                              + ar_member("/0", b"data"))
        output = io.BytesIO()
        decode(self.path, 1024, 1024, 2, output)
        stream = io.BytesIO(output.getvalue())
        found = []
        for index in range(2):
            size = struct.unpack(">Q", exact(stream, 8))[0]
            row = json.loads(exact(stream, size))
            self.assertEqual(row["ordinal"], index)
            found.append((row["path"], exact(stream, row["size"])))
        self.assertEqual(found, [("@ar-metadata/name-table", b"long-object-name.o/\n"),
                                 ("long-object-name.o", b"data")])
        self.assertEqual(stream.read(), bytes(8))
        self.assertEqual(list(self.path.parent.iterdir()), [self.path])

    def test_worker_refuses_missing_extra_and_size_mismatched_native_entries(self):
        self.path.write_bytes(ar_bytes([("one/", b"data")]))
        for headers in ([None], [("one", 3)], [("one", 4), ("extra", 0)]):
            reader = Mock()
            reader.header.side_effect = headers
            reader.chunks.return_value = [b"data"]
            with patch("image_secrets.ar_worker.ArReader", return_value=reader):
                with self.assertRaises(ScanError):
                    decode(self.path, 1024, 1024, 2, io.BytesIO())
            reader.close.assert_called_once()
        with patch("image_secrets.ar_worker.sys.argv", ["worker", str(self.path), "-1", "1", "1"]):
            self.assertEqual(main(), 2)

    def test_native_decoder_absence_warnings_and_partial_reads_refuse(self):
        with patch("image_secrets.ar_native.ctypes.util.find_library", return_value=None):
            with self.assertRaisesRegex(ScanError, "ar-decoder-unavailable"):
                ArReader(self.path)
        reader = object.__new__(ArReader)
        reader.handle = 1
        for status in (-30, -20, -10):
            reader.next = Mock(return_value=status)
            with self.assertRaises(ScanError):
                reader.header()
        for counts in ([0], [-1], [2], [1, 1]):
            reader.data = Mock(side_effect=counts)
            with self.assertRaises(ScanError):
                list(reader.chunks(1))
        reader.free = Mock(return_value=-1)
        with self.assertRaises(ScanError):
            reader.close()
        self.assertIsNone(reader.handle)

    def test_truncated_protocol_and_member_framing_refuse(self):
        with self.assertRaises(ScanError):
            exact(io.BytesIO(b"short"), 8)
        with self.assertRaises(ScanError):
            list(raw_chunks(io.BytesIO(b"short"), 0, 6))
        stream = MemberStream(io.BytesIO(b"dataNEXT"), 4)
        self.assertEqual(stream.read(100), b"data")
        self.assertEqual(stream.read(100), b"")
        self.assertEqual(stream.stream.read(), b"NEXT")

    def test_xz_concatenation_padding_truncation_and_dictionary_limit(self):
        payload = lzma.compress(b"one") + bytes(4) + lzma.compress(b"two")
        output = self.path.with_suffix(".decoded")
        self.path.write_bytes(payload)
        decode_xz(self.path, output, 6)
        self.assertEqual(output.read_bytes(), b"onetwo")
        for data, limit in ((payload, 5), (lzma.compress(b"data")[:-1], 100),
                               (lzma.compress(b"data") + bytes(1), 100),
                               (lzma.compress(b"data") + b"trailer", 100)):
            output.unlink(missing_ok=True)
            self.path.write_bytes(data)
            with self.assertRaises((ScanError, lzma.LZMAError)):
                decode_xz(self.path, output, limit)
        output.unlink(missing_ok=True)
        self.path.write_bytes(lzma.compress(b"data"))
        with patch("image_secrets.xz_stream.MAX_NESTED_MEMBER_BYTES", 1024):
            with self.assertRaises(lzma.LZMAError):
                decode_xz(self.path, output, 100)

    def test_nested_entry_and_container_caps_refuse(self):
        for kind, payload in (("zip", zip_bytes(b"data")), ("tar", tar_bytes([("entry", b"data")]))):
            self.path.write_bytes(payload)
            with self.assertRaisesRegex(ScanError, "entry-budget-exceeded"):
                decode_nested(RecordingStage(max_entries=0), self.path, self.location, 0, kind)
            with patch("image_secrets.containers.MAX_NESTED_MEMBER_BYTES", 10):
                with self.assertRaisesRegex(ScanError, "nested-container-size-limit"):
                    inspect_nested(RecordingStage(), self.path, self.location, 0)

    def test_zstd_and_xz_expansion_refuse_and_cleanup(self):
        payload = b"x" * 4096
        zstd = subprocess.run(["zstd", "-q", "-c"], input=payload, capture_output=True, check=True).stdout
        for kind, data in (("xz", lzma.compress(payload)), ("zstd", zstd)):
            self.path.write_bytes(data)
            with self.assertRaises(ScanError):
                decode_nested(RecordingStage(max_bytes=100), self.path, self.location, 0, kind)
            self.assertFalse(self.path.with_suffix(".decoded").exists())

    def test_elf_text_view_keeps_ascii_and_hash_and_charges_budget(self):
        raw = b"\x7fELF\x00\xff\tpublic-ascii\n"
        self.path.write_bytes(raw)
        with self.assertRaises(ScanError):
            prepare(self.path, Budget(len(raw) - 1, 1))
        self.assertEqual(self.path.read_bytes(), raw)
        budget = Budget(len(raw), 1)
        self.assertEqual(prepare(self.path, budget), digest(raw))
        self.assertEqual(self.path.read_bytes(), b"\nELF\n\n\tpublic-ascii\n")
        self.assertEqual(budget.bytes, len(raw))
        self.assertFalse(self.path.with_suffix(".view").exists())
