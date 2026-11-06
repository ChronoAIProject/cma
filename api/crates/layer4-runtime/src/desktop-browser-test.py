"""Linux regression: real launch scripts/IPC/processes, synthetic browser/windows."""
from concurrent.futures import ThreadPoolExecutor
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest

SOURCE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('browser', SOURCE / 'desktop-browser.py')
browser = importlib.util.module_from_spec(spec)
spec.loader.exec_module(browser)

FAKE = '''#!/usr/bin/env python3
import json, os, signal, subprocess, sys, time
from pathlib import Path
root = Path(os.environ['FIXTURE'])
if (root / 'fail').exists(): sys.exit(7)
if (root / 'pid').exists():
    pid = int((root / 'pid').read_text())
    try: os.kill(pid, 0)
    except ProcessLookupError: pass
    else:
        (root / 'forwarded').write_text(json.dumps(sys.argv[1:]))
        sys.exit(0)
(root / 'pid').write_text(str(os.getpid()))
(root / 'arguments').write_text(json.dumps(sys.argv[1:]))
(root / 'fds').write_text(json.dumps({p.name: os.readlink(p) for p in Path('/proc/self/fd').iterdir() if p.exists()}))
if (root / 'helper').exists():
    child = subprocess.Popen(['sleep', '120'], start_new_session=True)
    (root / 'helper-pid').write_text(str(child.pid))
while True: time.sleep(.1)
'''
PROBE = '''#!/usr/bin/env python3
import json, os, sys, time
from pathlib import Path
root = Path(os.environ['FIXTURE'])
if (root / 'hang').exists(): time.sleep(120)
if (root / 'timeout').exists(): sys.exit(1)
if (root / 'wrong-pid').exists():
    print(json.dumps([{'pid': 999999, 'class': 'chromium', 'address': '0x123'}]))
    sys.exit(0)
pid = int((root / 'pid').read_text()) if (root / 'pid').exists() else 0
if 'clients' in sys.argv:
    print(json.dumps([{'pid': pid, 'class': 'chromium', 'address': '0x123'}]))
elif 'search' in sys.argv:
    assert str(pid) == sys.argv[sys.argv.index('--pid') + 1]
    print('123')
else: (root / 'focused').touch()
'''


@unittest.skipUnless(sys.platform == 'linux', 'requires Linux desktop process ownership')
class Launcher(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='cma-browser-test-')
        self.root = Path(self.tmp.name)
        self.runtime = self.root / 'runtime'
        self.runtime.mkdir()
        (self.runtime / 'cma-hyprland-instance').write_text('fixture')
        self.env = dict(os.environ, HOME=str(self.root), FIXTURE=str(self.root),
                        PATH=str(self.root) + ':' + os.environ['PATH'])
        for name, source in [('fake.py', FAKE), ('xdotool', PROBE), ('hyprctl', PROBE)]:
            p = self.root / name
            p.write_text(source)
            p.chmod(0o755)
        (self.root / 'chromium').symlink_to(sys.executable)
        # Configure only the fixture copy; production paths and deadline are fixed.
        helper = self.root / 'manager.py'
        helper.write_text((SOURCE / 'desktop-browser.py').read_text().replace(
            "RUNTIME = Path('/tmp/cma-desktop-runtime')", f'RUNTIME = Path({str(self.runtime)!r})')
            .replace('TIMEOUT = 20', 'TIMEOUT = 1')
            .replace("command = ['chromium']", f"command = ['chromium', {str(self.root / 'fake.py')!r}]"))
        self.owner = subprocess.Popen(['sleep', '120'], start_new_session=True)
        self.manager = None
        self.helper = helper

    def start(self, wayland=False):
        self.manager = subprocess.Popen([sys.executable, str(self.helper), '--serve',
                                        str(self.owner.pid), *(['--wayland'] if wayland else [])],
                                       env=self.env, stdin=subprocess.DEVNULL,
                                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        self.wait(lambda: (self.runtime / 'browser.sock').exists())

    def launch(self, wayland=False, args=(), pass_fds=()):
        script = self.root / 'session.sh'
        script.write_text((SOURCE / ('omarchy/session.sh' if wayland else 'desktop-session.sh'))
                          .read_text().replace('/usr/local/bin/cma-desktop-browser', str(self.helper)))
        # communicate() must reach EOF on both pipes while the browser is alive.
        return subprocess.run(['bash', str(script), '--browser', *args], env=self.env,
                              capture_output=True, text=True, timeout=5, pass_fds=pass_fds)

    def wait(self, condition):
        deadline = time.monotonic() + 4
        while not condition():
            self.assertLess(time.monotonic(), deadline, 'fixture deadline exceeded')
            time.sleep(.03)

    def pid(self):
        return int((self.root / 'pid').read_text())

    def tearDown(self):
        self.owner.kill()
        self.owner.wait(timeout=3)
        if self.manager:
            self.manager.wait(timeout=5)
            stderr = self.manager.stderr.read().decode()
            self.manager.stderr.close()
            self.assertEqual(self.manager.returncode, 0, stderr)
        self.tmp.cleanup()

    def test_launch_returns_with_browser_and_pipes_closed(self):
        self.start()
        read_fd, write_fd = os.pipe()
        try:
            result = self.launch(args=['https://example.invalid/fixture'], pass_fds=(write_fd,))
            self.assertEqual(result.returncode, 0, result.stderr)
            pid = self.pid()
            self.assertTrue(browser.identity(pid))
            self.assertEqual(os.getsid(pid), pid)
            self.assertEqual(os.getpgid(pid), pid)
            os.close(write_fd)
            write_fd = None
            os.set_blocking(read_fd, False)
            self.assertEqual(os.read(read_fd, 1), b'')
            fds = json.loads((self.root / 'fds').read_text())
            self.assertEqual(fds['0'], '/dev/null')
            self.assertTrue(fds['1'].endswith('cma-chromium.log'))
            self.assertEqual(fds['1'], fds['2'])
            self.assertFalse(any(value.startswith('pipe:') for value in fds.values()))
            args = json.loads((self.root / 'arguments').read_text())
            self.assertIn('--user-data-dir=' + str(self.root / '.cma/chrome'), args)
        finally:
            os.close(read_fd)
            if write_fd is not None: os.close(write_fd)

    def test_wayland_readiness_and_profile(self):
        self.start(wayland=True)
        result = self.launch(wayland=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('--ozone-platform=wayland', json.loads((self.root / 'arguments').read_text()))

    def test_already_running_focus_and_url_reuse(self):
        self.start()
        self.assertEqual(self.launch().returncode, 0)
        pid = self.pid()
        self.assertEqual(self.launch().returncode, 0)
        self.assertTrue((self.root / 'focused').exists())
        self.assertEqual(self.launch(args=['https://example.invalid/next']).returncode, 0)
        self.assertEqual(self.pid(), pid)
        self.assertIn('https://example.invalid/next', json.loads((self.root / 'forwarded').read_text()))

    def test_concurrent_requests_reuse_one_browser(self):
        self.start()
        def request(_):
            return subprocess.run([sys.executable, str(self.helper)], env=self.env,
                                  capture_output=True, timeout=5).returncode
        with ThreadPoolExecutor(max_workers=4) as pool:
            self.assertEqual(list(pool.map(request, range(4))), [0, 0, 0, 0])
        self.assertTrue(browser.identity(self.pid()))
        self.assertFalse((self.root / 'forwarded').exists())

    def test_other_wayland_window_does_not_prove_readiness(self):
        (self.root / 'wrong-pid').touch()
        self.start(wayland=True)
        self.assertEqual(self.launch(wayland=True).returncode, 124)
        self.assertIsNone(browser.identity(self.pid()))

    def test_browser_start_failure(self):
        (self.root / 'fail').touch()
        self.start()
        result = self.launch()
        self.assertEqual(result.returncode, 70, result.stderr)

    def test_readiness_timeout_stops_browser(self):
        (self.root / 'timeout').touch()
        self.start()
        result = self.launch()
        self.assertEqual(result.returncode, 124, result.stderr)
        self.assertIsNone(browser.identity(self.pid()))

    def test_hanging_probe_is_bounded(self):
        (self.root / 'hang').touch()
        self.start()
        self.assertEqual(self.launch().returncode, 124)
        self.assertIsNone(browser.identity(self.pid()))

    def test_desktop_sigkill_cleans_browser_and_separate_session_helper(self):
        (self.root / 'helper').touch()
        self.start()
        self.assertEqual(self.launch().returncode, 0)
        helper = int((self.root / 'helper-pid').read_text())
        pid = self.pid()
        os.killpg(self.owner.pid, signal.SIGKILL)
        self.wait(lambda: browser.identity(pid) is None and browser.identity(helper) is None)
        self.manager.wait(timeout=4)
        self.assertFalse((self.runtime / 'browser.sock').exists())

    def test_profile_override_refused(self):
        self.start()
        self.assertEqual(self.launch(args=['--user-data-dir=/tmp/other']).returncode, 64)
        self.assertFalse((self.root / 'pid').exists())

    def test_missing_desktop_returns_unavailable(self):
        self.assertEqual(self.launch().returncode, 69)


if __name__ == '__main__':
    unittest.main()
