"""No tar traversal may silently stop before unexplained physical content."""

import gzip
import os
import unittest

from tests import test_runtime_archives as runtime_tests
from tests.fixtures import docker_archive, tar_bytes


class TarIntegrityTests(unittest.TestCase):
    setUp = runtime_tests.RuntimeArchiveScanTests.setUp
    tearDown = runtime_tests.RuntimeArchiveScanTests.tearDown
    scan = runtime_tests.RuntimeArchiveScanTests.scan
    image = runtime_tests.RuntimeArchiveScanTests.image

    def test_empty_and_benign_padding_remain_clean(self):
        for data in (bytes(1024), tar_bytes([("one", b"ordinary")]),
                     tar_bytes([("one", b"ordinary")]) + bytes(512)):
            report, _ = self.scan(self.image(data), 0)
            self.assertTrue(report["complete"])

    def test_appended_member_and_nested_secret_refuse(self):
        value = "ghp_" + os.urandom(18).hex()
        secret = ("token=" + value + "\n").encode()
        first = tar_bytes([("one", b"ordinary")])
        for second in (tar_bytes([("two", b"ordinary")]),
                       tar_bytes([("hidden", gzip.compress(secret))])):
            report, output = self.scan(self.image(first + second), 2)
            self.assertEqual(report["error"], "tar-nonzero-trailer")
            self.assertFalse(report["complete"])
            self.assertNotIn(value, output)

    def test_malformed_missing_and_partial_trailers_refuse(self):
        first = tar_bytes([("one", b"ordinary")])
        for data in (first + b"x", first + bytes(1), first[:1024], first[:1536],
                     first[:1024] + b"invalid" + bytes(1017)):
            report, _ = self.scan(self.image(data), 2)
            self.assertFalse(report["complete"])

    def test_layer_and_outer_archive_use_the_same_trailer_guard(self):
        trailing = tar_bytes([("hidden", gzip.compress(b"ordinary"))])
        archive = self.root / "image.tar"
        docker_archive(archive, [tar_bytes([("one", b"ordinary")]) + trailing])
        report, _ = self.scan(archive, 2)
        self.assertEqual(report["error"], "tar-nonzero-trailer")
        docker_archive(archive, [tar_bytes([("one", b"ordinary")])])
        archive.write_bytes(archive.read_bytes() + trailing)
        report, _ = self.scan(archive, 2)
        self.assertEqual(report["error"], "tar-nonzero-trailer")
