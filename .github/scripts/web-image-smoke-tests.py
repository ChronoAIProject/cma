#!/usr/bin/env python3
"""Exercise watchdog exit status, evidence and process cleanup with real children."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

spec = importlib.util.spec_from_file_location(
    "web_image_smoke", Path(__file__).with_name("web-image-smoke.py"))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


class BrowserWatchdogTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def run_child(self, source, timeout=1, grace=1):
        smoke.run_browser([sys.executable, "-c", source], cwd=self.root,
                          env=os.environ.copy(), evidence=self.root,
                          timeout=timeout, grace=grace)

    def evidence(self):
        return json.loads((self.root / "browser-process.json").read_text())

    def test_success(self):
        self.run_child("pass")
        self.assertEqual(self.evidence()["returncode"], 0)
        self.assertFalse(self.evidence()["timed_out"])

    def test_failure_is_propagated(self):
        with self.assertRaises(subprocess.CalledProcessError) as error:
            self.run_child("raise SystemExit(7)")
        self.assertEqual(error.exception.returncode, 7)
        self.assertEqual(self.evidence()["returncode"], 7)

    def test_interrupt_flushes_artifacts_but_cannot_turn_timeout_into_success(self):
        with self.assertRaises(subprocess.TimeoutExpired):
            self.run_child("""
import json, signal, time
from pathlib import Path
def stop(*_):
    assert json.loads(Path('browser-process.json').read_text())['timed_out']
    Path('retained-artifact').write_text('flushed before exit')
    raise SystemExit(0)
signal.signal(signal.SIGINT, stop)
while True: time.sleep(.01)
""")
        self.assertTrue((self.root / "retained-artifact").exists())
        self.assertTrue(self.evidence()["timed_out"])
        self.assertFalse(self.evidence()["forced_kill"])
        self.assertEqual(self.evidence()["returncode"], 0)

    def test_unresponsive_runner_and_descendant_are_killed(self):
        with self.assertRaises(subprocess.TimeoutExpired):
            self.run_child("""
import signal, subprocess, sys, time
signal.signal(signal.SIGINT, signal.SIG_IGN)
subprocess.Popen([sys.executable, '-c', '''
import time
with open('heartbeat', 'a', buffering=1) as stream:
    while True:
        stream.write('tick' + chr(10))
        time.sleep(.02)
'''])
while True: time.sleep(.01)
""", grace=.2)
        self.assertTrue(self.evidence()["forced_kill"])
        self.assertLess(self.evidence()["returncode"], 0)
        # A live orphan keeps writing after its parent is reaped.
        before = (self.root / "heartbeat").read_text()
        self.assertTrue(before)
        time.sleep(.2)
        self.assertEqual((self.root / "heartbeat").read_text(), before)

    @unittest.skipUnless(sys.platform == "linux", "requires orphan adoption")
    def test_exit_with_unsampled_detached_descendants_and_unrelated_child(self):
        control = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"],
                                   start_new_session=True)
        try:
            for status in (0, 7):
                source = """
import subprocess, sys
subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'],
                 start_new_session=True)
raise SystemExit(STATUS)
""".replace("STATUS", str(status))
                if status:
                    with self.assertRaises(subprocess.CalledProcessError):
                        self.run_child(source)
                else:
                    self.run_child(source)
                result = self.evidence()
                self.assertEqual(result["returncode"], status)
                self.assertFalse(result["timed_out"])
                self.assertEqual(result["process_cleanup"]["remaining"], [])
                self.assertGreaterEqual(len(result["process_cleanup"]["owned"]), 2)
                self.assertIsNone(control.poll())
        finally:
            control.kill()
            control.wait()

    def test_launch_error_is_not_reported_as_a_test_pass(self):
        with self.assertRaises(subprocess.CalledProcessError):
            smoke.run_browser([str(self.root / "missing-executable")], cwd=self.root,
                              env=os.environ.copy(), evidence=self.root, timeout=1)
        self.assertFalse((self.root / "browser-process.json").exists())


if __name__ == "__main__":
    unittest.main()
