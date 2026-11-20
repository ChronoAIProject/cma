"""Metadata entry/byte limits precede eager archive parser allocation."""

import io
import struct
import tarfile
import unittest
import zipfile
from unittest.mock import patch

from image_secrets.common import ScanError
from image_secrets.container_limits import BoundedReader, zip_directory
from image_secrets.containers import decode_nested
from tests import test_runtime_archives as fixtures


class ContainerLimitTests(unittest.TestCase):
    setUp = fixtures.ArDecoderTests.setUp

    def test_zip_entry_cap_and_directory_cap_precede_stdlib_parser(self):
        self.path.write_bytes(fixtures.zip_bytes(b"ordinary data"))
        with patch("image_secrets.containers.zipfile.ZipFile") as parser:
            with self.assertRaisesRegex(ScanError, "entry-budget-exceeded"):
                decode_nested(fixtures.RecordingStage(max_entries=0), self.path, self.location, 0, "zip")
            parser.assert_not_called()
        with patch("image_secrets.container_limits.MAX_ZIP_DIRECTORY_BYTES", 1):
            with self.assertRaisesRegex(ScanError, "nested-zip-directory-size-limit"):
                zip_directory(self.path, 100)

    def test_false_eocd_count_and_multidisk_refuse(self):
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            for i in range(3):
                archive.writestr(str(i), b"data")
        data = bytearray(buffer.getvalue())
        struct.pack_into("<HH", data, len(data) - 14, 1, 1)
        self.path.write_bytes(data)
        with self.assertRaisesRegex(ScanError, "entry-budget-exceeded"):
            zip_directory(self.path, 2)
        with self.assertRaisesRegex(ScanError, "nested-zip-directory-invalid"):
            zip_directory(self.path, 100)
        struct.pack_into("<H", data, len(data) - 18, 1)
        self.path.write_bytes(data)
        with self.assertRaises(ScanError):
            zip_directory(self.path, 100)

    def test_oversized_pax_metadata_refuses_and_read_allocations_are_bounded(self):
        entry = tarfile.TarInfo("pax-metadata")
        entry.type = tarfile.XHDTYPE
        entry.size = 512 * 1024**2
        self.path.write_bytes(entry.tobuf(format=tarfile.USTAR_FORMAT))
        # Newer Python reads metadata in small chunks and rejects this truncated
        # fixture first; older versions request its entire declared extent.
        with self.assertRaises((ScanError, tarfile.TarError)):
            decode_nested(fixtures.RecordingStage(), self.path, self.location, 0, "tar")
        reader = BoundedReader(io.BytesIO(b"data"))
        with self.assertRaisesRegex(ScanError, "nested-metadata-size-limit"):
            reader.read(512 * 1024**2)
        with self.assertRaises(ScanError):
            reader.read()
        self.assertEqual(reader.read(4), b"data")
