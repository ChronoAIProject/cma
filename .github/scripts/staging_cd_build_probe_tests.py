"""Cancellation reaps an in-flight ownership probe and only its own build."""
import contextlib
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

SLEEP_WITH_HELPER = """
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

signal.signal(signal.SIGTERM, signal.SIG_IGN)
helper = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
marker = Path(sys.argv[1])
partial = marker.with_suffix(".tmp")
partial.write_text(f"{os.getpid()} {helper.pid}")
partial.replace(marker)
time.sleep(30)
"""

PUBLISHER = r"""
from pathlib import Path
import sys
from staging_cd_build_guard import run

def check_owner(probe):
    if Path("build.pid").exists():
        probe([sys.executable, "-c", sys.argv[1], "probe.pid"], timeout=20,
              capture_output=True, text=True, check=False)
    else:
        result = probe([sys.executable, "-c", "print('current')"], timeout=5,
                       capture_output=True, text=True, check=False)
        assert result.returncode == 0 and result.stdout == "current\n"

run([sys.executable, "-c", sys.argv[1], "build.pid"], cwd=None, env={}, timeout=25,
    check_owner=check_owner, interval=.05)
"""


class BuildProbeTests(unittest.TestCase):
    def test_cancels_and_reaps_an_in_flight_ownership_probe_with_its_build(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = {"PYTHONPATH": str(Path(__file__).resolve().parent)}
            with subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"],
                                  start_new_session=True) as sibling, subprocess.Popen(
                [sys.executable, "-B", "-c", PUBLISHER, SLEEP_WITH_HELPER], cwd=root, env=env,
            ) as publishing:
                groups = (root / "build.pid", root / "probe.pid")
                try:
                    deadline = time.monotonic() + 5
                    while publishing.poll() is None and not groups[1].exists() and time.monotonic() < deadline:
                        time.sleep(.01)
                    self.assertTrue(groups[1].exists(), "the periodic ownership command did not start")
                    publishing.terminate()
                    self.assertEqual(publishing.wait(timeout=7), -signal.SIGTERM)
                    for group in groups:
                        for pid in map(int, group.read_text().split()):
                            with self.assertRaises(ProcessLookupError):
                                os.kill(pid, 0)
                    self.assertIsNone(sibling.poll())
                finally:
                    if publishing.poll() is None:
                        publishing.kill()
                    for group in groups:
                        if group.exists():
                            with contextlib.suppress(ProcessLookupError):
                                os.killpg(int(group.read_text().split()[0]), signal.SIGKILL)
                    sibling.terminate()


if __name__ == "__main__":
    unittest.main()
