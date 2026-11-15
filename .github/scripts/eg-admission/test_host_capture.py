import json
from pathlib import Path
import tempfile
import unittest

from host_capture import capture, collect_directory, checked_metadata

ROOT = Path(__file__).resolve().parent
IDENTITY = "a" * 64
EXECUTABLE = "b" * 64


class HostCaptureTests(unittest.TestCase):
    def test_local_capture_preserves_reviewed_records_without_a_pod_claim(self):
        raw = (ROOT / "fixtures/tracing-format.txt").read_bytes()
        packet = capture(raw, IDENTITY)
        self.assertEqual(packet["capture_kind"], "local_child_process")
        self.assertEqual(packet["host_capture_sha256"], IDENTITY)
        self.assertNotIn("pod_uid_sha256", packet)
        self.assertEqual(len(packet["observations"]), 3)
        self.assertEqual(packet["rejected_records"], 0)

    def test_private_logs_and_malformed_observations_are_not_exported(self):
        raw = b'private payload\nINFO wrong: message event="eg_admission_record"\n'
        packet = capture(raw, IDENTITY)
        self.assertEqual(packet["observations"], [])
        self.assertEqual(packet["rejected_records"], 1)
        self.assertNotIn("private payload", json.dumps(packet))

    def fixture(self, root):
        temporary, evidence = root / "temporary", root / "evidence"
        temporary.mkdir()
        evidence.mkdir()
        child = temporary / "cmaeg-runner-fixture"
        child.mkdir()
        metadata = {"replicas": 1, "enabled": False, "ordinal": 1,
                    "process_sha256": IDENTITY, "executable_sha256": EXECUTABLE, "joined": True}
        (child / "capture.json").write_text(json.dumps(metadata))
        (child / "trace.log").write_bytes((ROOT / "fixtures/tracing-format.txt").read_bytes())
        return temporary, evidence, child, metadata

    def test_parent_capture_seals_metadata_and_never_removes_input(self):
        with tempfile.TemporaryDirectory() as root:
            temporary, evidence, child, _ = self.fixture(Path(root))
            receipt = collect_directory(temporary, evidence, EXECUTABLE)
            self.assertTrue(receipt["complete"])
            self.assertTrue((child / "trace.log").exists())
            packet = json.loads((evidence / "n1-false-1.json").read_text())
            self.assertEqual(packet["host_process"]["executable_sha256"], EXECUTABLE)
            self.assertTrue(packet["group_exit_confirmed"])
            self.assertEqual(json.loads((evidence / "capture-receipt.json").read_text()), receipt)

    def test_failure_receipt_survives_missing_input(self):
        with tempfile.TemporaryDirectory() as root:
            temporary, evidence, child, _ = self.fixture(Path(root))
            (child / "trace.log").unlink()
            receipt = collect_directory(temporary, evidence, EXECUTABLE)
            self.assertFalse(receipt["complete"])
            self.assertEqual(receipt["failures"], ["capture_child_failed"])
            self.assertTrue(child.exists())
            self.assertTrue((evidence / "capture-receipt.json").exists())

    def test_forged_metadata_is_refused_without_copying_raw_data(self):
        with tempfile.TemporaryDirectory() as root:
            _, _, _, metadata = self.fixture(Path(root))
            for key, value in [("ordinal", 2), ("replicas", True), ("enabled", 1),
                               ("executable_sha256", "c" * 64), ("process_sha256", "private")]:
                with self.subTest(key=key), self.assertRaises(ValueError):
                    checked_metadata(json.dumps({**metadata, key: value}), EXECUTABLE)


if __name__ == "__main__":
    unittest.main()
