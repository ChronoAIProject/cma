import hashlib
import json
import os
from pathlib import Path
import runpy
import subprocess
import tempfile
import unittest
from unittest.mock import patch


SCAN = runpy.run_path(str(Path(__file__).with_name("omarchy-ci-scan.py")))
REVISION = "a" * 40
IMAGE_ID = "sha256:" + "b" * 64
ARCHIVE = b"synthetic image archive"


class ScanTests(unittest.TestCase):
    def setUp(self):
        runner = self.enterContext(tempfile.TemporaryDirectory())
        self.root = Path(runner)
        self.source = self.root / "omarchy-ci-scan-input"
        self.destination = self.root / "ci-native-image"
        self.state = self.root / "omarchy-ci-scan-state"
        self.result = self.state / "result.json"
        self.source.mkdir()
        self.state.mkdir()
        (self.state / "scanner.log").touch()
        (self.state / "deadline").write_text("0")
        self.metadata = {
            "component": "runtime-omarchy",
            "sourceRevision": REVISION,
            "archive": SCAN["ARCHIVE"],
            "archiveSha256": hashlib.sha256(ARCHIVE).hexdigest(),
            "archiveBytes": len(ARCHIVE),
            "imageId": IMAGE_ID,
            "testedReference": "cma-runtime-omarchy:ci",
            "workflow": {"runId": "1", "runAttempt": "2"},
        }
        self.metadata_path = self.source / "metadata.json"
        self.metadata_path.write_text(json.dumps(self.metadata))
        self.archive_path = self.source / SCAN["ARCHIVE"]
        self.archive_path.write_bytes(ARCHIVE)
        self.enterContext(patch.dict(os.environ, {"GITHUB_RUN_ID": "1", "GITHUB_RUN_ATTEMPT": "2"}))
        self.inspect = self.enterContext(patch.object(subprocess, "check_output", return_value=IMAGE_ID + "\n"))

    def finished(self, code, image=None):
        self.result.write_text(json.dumps({"image": self.metadata if image is None else image, "exit_code": code}))

    def promote(self):
        SCAN["promote"](self.source, self.state, self.destination, REVISION)

    def rejects(self, message):
        with self.assertRaisesRegex(ValueError, message):
            self.promote()
        self.assertFalse(self.destination.exists())

    def test_joins_success_and_retains_the_original_archive_and_metadata(self):
        metadata = self.metadata_path.read_bytes()
        self.finished(0)
        self.promote()
        self.assertEqual((self.destination / SCAN["ARCHIVE"]).read_bytes(), ARCHIVE)
        self.assertEqual((self.destination / "metadata.json").read_bytes(), metadata)
        self.assertFalse(self.source.exists())

    def test_existing_scanner_is_bounded_and_records_terminal_result(self):
        with patch.object(subprocess, "run", return_value=subprocess.CompletedProcess([], 124)) as command:
            SCAN["scan"](self.source, self.state, REVISION)
        args, = command.call_args.args
        self.assertEqual(args[:3], ["timeout", "--kill-after=5", "1800"])
        self.assertTrue(args[5].endswith("/ci_image_secret_scan.py"))
        self.assertEqual(args[6:], ["runtime-omarchy", str(self.source), REVISION])
        self.rejects("image secret scan failed")

    def test_refuses_missing_result_and_foreground_archive_replacement(self):
        self.rejects("did not record a result")
        self.finished(0)
        self.archive_path.write_bytes(b"x" * len(ARCHIVE))
        self.rejects("archive changed")

    def test_refuses_changed_loaded_image_or_result_from_another_image(self):
        self.finished(0, dict(self.metadata, imageId="sha256:" + "d" * 64))
        self.rejects("describes a different image")
        self.finished(0)
        self.inspect.return_value = "sha256:" + "d" * 64
        self.rejects("tested image changed")

    def test_refuses_stale_run(self):
        self.finished(0)
        with patch.dict(os.environ, {"GITHUB_RUN_ATTEMPT": "3"}):
            self.rejects("belongs to another image or run")


if __name__ == "__main__":
    unittest.main()
