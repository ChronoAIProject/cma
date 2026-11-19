"""Malformed or partial worker output cannot become a successful archive scan."""

import io
import json
import struct
import unittest
from unittest.mock import Mock, patch

from image_secrets.ar_decoder import decode_ar
from image_secrets.common import ScanError
from image_secrets.containers import child_location
from tests import test_runtime_archives as fixtures


def framed(row, data=b""):
    header = json.dumps(row).encode()
    return struct.pack(">Q", len(header)) + header + data


class WorkerProtocolTests(unittest.TestCase):
    setUp = fixtures.ArDecoderTests.setUp

    def test_invalid_frames_truncation_and_failure_after_eof_refuse(self):
        row = {"ordinal": 0, "path": "member", "size": 4}
        variants = [
            (b"short", 0), (struct.pack(">Q", 65537), 0),
            (framed({}), 0), (framed(dict(row, ordinal=True)), 0),
            (framed(dict(row, ordinal=1)), 0), (framed(dict(row, size=-1)), 0),
            (framed(dict(row, size=1025)), 0), (framed(dict(row, path="../outside")), 0),
            (framed(row, b"abc"), 0), (bytes(8) + b"trailing", 0), (bytes(8), 2),
        ]
        for payload, status in variants:
            process = Mock(stdout=io.BytesIO(payload))
            process.poll.return_value = process.wait.return_value = status
            with patch("image_secrets.ar_decoder.subprocess.Popen", return_value=process):
                with self.assertRaises(ScanError):
                    decode_ar(fixtures.RecordingStage(), self.path, self.location, 0, 1024, child_location)
            self.assertTrue(process.stdout.closed)

    def test_success_protocol_still_requires_consuming_every_member_byte(self):
        row = {"ordinal": 0, "path": "member", "size": 4}
        process = Mock(stdout=io.BytesIO(framed(row, b"data") + bytes(8)))
        process.poll.return_value = process.wait.return_value = 0
        stage = fixtures.RecordingStage()
        stage.file = Mock()
        with patch("image_secrets.ar_decoder.subprocess.Popen", return_value=process):
            with self.assertRaises(ScanError):
                decode_ar(stage, self.path, self.location, 0, 1024, child_location)
        self.assertTrue(process.stdout.closed)
