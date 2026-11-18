"""A watchdog must terminate descendants as well as its direct child."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import Mock, patch
import signal

from staging_cd_process import ProcessTimeout, run


class ProcessTests(unittest.TestCase):
    def test_controller_gets_sigterm_and_the_full_fifteen_second_cleanup_allowance(self):
        child = Mock(pid=1234)
        child.__enter__ = Mock(return_value=child)
        child.__exit__ = Mock(return_value=False)
        for code in (20, 30):
            child.wait.side_effect = [subprocess.TimeoutExpired("controller", 600), code]
            child.wait.reset_mock()
            with patch("staging_cd_process.subprocess.Popen", return_value=child), \
                    patch("staging_cd_process.os.killpg") as kill, self.assertRaises(ProcessTimeout) as caught:
                run(["controller"], {}, 600, grace_seconds=15)
            self.assertEqual(caught.exception.returncode, code)
            kill.assert_called_once_with(1234, signal.SIGTERM)
            self.assertEqual([call.kwargs["timeout"] for call in child.wait.call_args_list], [600, 15])
        child.wait.side_effect = [subprocess.TimeoutExpired("controller", 600),
                                 subprocess.TimeoutExpired("cleanup", 15), -9]
        with patch("staging_cd_process.subprocess.Popen", return_value=child), \
                patch("staging_cd_process.os.killpg") as kill, self.assertRaises(ProcessTimeout):
            run(["controller"], {}, 600, grace_seconds=15)
        self.assertEqual([call.args[1] for call in kill.call_args_list], [signal.SIGTERM, signal.SIGKILL])

    def test_real_controller_cleanup_is_joined_and_stubborn_controller_is_killed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            script = root / "controller.py"
            script.write_text("import signal, sys, time\nfrom pathlib import Path\n"
                              "def stop(*_):\n time.sleep(0.05)\n Path('reaped').touch()\n sys.exit(30)\n"
                              "signal.signal(signal.SIGTERM, stop)\ntime.sleep(5)\n")
            with self.assertRaises(ProcessTimeout) as caught:
                run([sys.executable, str(script)], {}, 0.2, cwd=root, grace_seconds=0.5)
            self.assertEqual(caught.exception.returncode, 30)
            self.assertTrue((root / "reaped").exists())
            script.write_text("import signal, time\nsignal.signal(signal.SIGTERM, signal.SIG_IGN)\ntime.sleep(5)\n")
            with self.assertRaises(ProcessTimeout) as caught:
                run([sys.executable, str(script)], {}, 0.2, cwd=root, grace_seconds=0.1)
            self.assertEqual(caught.exception.returncode, -signal.SIGKILL)

    def test_timeout_kills_the_whole_group_before_a_descendant_can_write(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            child = root / "child.py"
            child.write_text("import time\nfrom pathlib import Path\ntime.sleep(0.8)\nPath('late').write_text('unexpected')\n")
            parent = root / "parent.py"
            parent.write_text("import subprocess, sys, time\nsubprocess.Popen([sys.executable, 'child.py'])\ntime.sleep(5)\n")
            with self.assertRaises(subprocess.TimeoutExpired):
                run([sys.executable, str(parent)], {"PATH": os.environ["PATH"]}, 0.2, cwd=root)
            time.sleep(0.9)
            self.assertFalse((root / "late").exists())
            self.assertEqual(run([sys.executable, "-c", "raise SystemExit(10)"], {}, 2), 10)


if __name__ == "__main__":
    unittest.main()
