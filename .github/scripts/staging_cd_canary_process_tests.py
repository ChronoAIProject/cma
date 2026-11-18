"""Real descendants cannot outlive the canary runner's cleanup proof."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

from staging_cd_build import command
from staging_cd_canary_process import CanaryProcessFailure, run


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def wait_file(path):
    end = time.monotonic() + 5
    while not path.exists() and time.monotonic() < end:
        time.sleep(0.01)
    if not path.exists():
        raise AssertionError("fixture did not become ready")


class CanaryProcessTests(unittest.TestCase):
    def fixture(self, root, *, leader=None):
        helper = root / "helper.py"
        helper.write_text(
            "import os, signal, time\nfrom pathlib import Path\n"
            "signal.signal(signal.SIGTERM, signal.SIG_IGN)\n"
            f"Path({str(root / 'helper.pid')!r}).write_text(str(os.getpid()))\n"
            "time.sleep(1.5)\n"
            f"Path({str(root / 'late-resource')!r}).touch()\n"
            "time.sleep(30)\n")
        parent = root / "parent.py"
        parent.write_text(
            "import subprocess, sys, time\nfrom pathlib import Path\n"
            f"subprocess.Popen([sys.executable, {str(helper)!r}])\n"
            f"while not Path({str(root / 'helper.pid')!r}).exists(): time.sleep(0.01)\n"
            + ("time.sleep(30)\n" if leader is None else f"sys.exit({leader})\n"))
        self.addCleanup(self.cleanup_fixture, root)
        return [sys.executable, "-B", str(parent)]

    def cleanup_fixture(self, root):
        pidfile = root / "helper.pid"
        if pidfile.exists():
            pid = int(pidfile.read_text())
            if alive(pid):
                os.kill(pid, signal.SIGKILL)

    def assert_gone(self, root):
        self.assertFalse(alive(int((root / "helper.pid").read_text())))
        self.assertFalse((root / "late-resource").exists())

    def test_success_returns_output_and_excludes_credentials(self):
        with patch.dict(os.environ, {"GH_TOKEN": "private-fixture-value"}):
            result = command([sys.executable, "-c", "import os; print(os.getenv('GH_TOKEN', 'absent'))"],
                             env={"CMA_RUNTIME_EVIDENCE_ID": "fixture"}, timeout=2)
        self.assertEqual(result, b"absent\n")

    def test_timeout_kills_and_reaps_stubborn_descendant_before_return(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            with self.assertRaises(CanaryProcessFailure) as raised:
                run(self.fixture(root), dict(os.environ), 0.3, grace=0.05)
            self.assertEqual(raised.exception.reason, "timeout")
            self.assertTrue(raised.exception.quiesced)
            self.assert_gone(root)

    def test_leader_success_or_failure_with_live_descendant_is_not_success(self):
        for code in (0, 7):
            with self.subTest(code=code), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                with self.assertRaises(CanaryProcessFailure) as raised:
                    run(self.fixture(root, leader=code), dict(os.environ), 2, grace=0.05)
                self.assertEqual(raised.exception.reason, "surviving_helper" if code == 0 else "abnormal_exit")
                self.assertTrue(raised.exception.quiesced)
                self.assert_gone(root)

    def test_sigterm_and_sigint_kill_descendants_and_restore_handlers(self):
        for sig in (signal.SIGTERM, signal.SIGINT):
            with self.subTest(signal=sig), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                args = self.fixture(root)
                wrapper = root / "wrapper.py"
                wrapper.write_text(
                    "import json, os, signal, sys\nfrom pathlib import Path\n"
                    f"sys.path.insert(0, {str(Path(__file__).resolve().parent)!r})\n"
                    "from staging_cd_canary_process import CanaryProcessFailure, run\n"
                    "before = signal.getsignal(signal.SIGTERM)\n"
                    "try:\n"
                    f"    run({args!r}, dict(os.environ), 20, grace=0.05)\n"
                    "except CanaryProcessFailure as e:\n"
                    f"    Path({str(root / 'result')!r}).write_text(json.dumps([e.reason, e.quiesced, before == signal.getsignal(signal.SIGTERM)]))\n")
                child = subprocess.Popen([sys.executable, "-B", str(wrapper)], stdout=subprocess.DEVNULL,
                                         stderr=subprocess.DEVNULL)
                try:
                    wait_file(root / "helper.pid")
                    child.send_signal(sig)
                    self.assertEqual(child.wait(timeout=6), 0)
                    self.assertEqual(json.loads((root / "result").read_text()), ["cancelled", True, True])
                    self.assert_gone(root)
                finally:
                    if child.poll() is None:
                        child.kill()
                        child.wait(timeout=2)

    def test_uncertain_group_cleanup_cannot_report_quiescence(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            args = self.fixture(root)
            # Real processes are killed/reaped, but a failed final inventory
            # must still refuse to assert that the group has disappeared.
            from staging_cd_canary_process import wait_empty
            def uncertain(child, seconds):
                wait_empty(child, seconds)
                return False
            with patch("staging_cd_canary_process.wait_empty", side_effect=uncertain):
                with self.assertRaises(CanaryProcessFailure) as raised:
                    run(args, dict(os.environ), 0.3, grace=0.05)
            self.assertFalse(raised.exception.quiesced)
            self.assert_gone(root)

    def test_failure_output_and_launch_exception_remain_private(self):
        for args in ([sys.executable, "-c", "raise RuntimeError('private-canary-value')"],
                     ["/nonexistent-canary-helper"]):
            with self.assertRaises(CanaryProcessFailure) as raised:
                run(args, dict(os.environ), 2)
            self.assertNotIn("private-canary-value", str(raised.exception))
            self.assertTrue(raised.exception.quiesced)
        with self.assertRaises(CanaryProcessFailure):
            run([sys.executable], {}, 0)
