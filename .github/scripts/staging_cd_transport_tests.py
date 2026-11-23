"""Verify the web runner shortcut and its package-manager fallback."""
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("staging_cd_transport.sh")
# Package fetches repeat transient mirror failures, and every call is bounded
# (request timeouts, dpkg lock wait, wall clock) before publication gives up.
BOUNDED = ("apt-get -o Acquire::Retries=3 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30 "
           "-o DPkg::Lock::Timeout=60")
UPDATE = [BOUNDED + " update -qq"]
INSTALL = [BOUNDED + " install -y -qq skopeo libarchive-tools zstd"]
# The runner's coreutils: `timeout` runs the bounded command, `sleep` waits between attempts.
TIMEOUT = 'while [ $# -gt 0 ]; do case "$1" in --*) shift;; [0-9]*) shift; break;; *) break;; esac; done\nexec "$@"'
SLEEP = 'exit 0'


class TransportTests(unittest.TestCase):
    def check_transport(self, component, skopeo=None, apt_status=0, *, zstd="exit 0", python="exit 0",
                        fail_first=False):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            # fail_first: only the first apt call fails, as a dropped mirror connection would.
            status = (f'[ -e "$CALLS.failed" ] && exit 0; touch "$CALLS.failed"; exit {apt_status}'
                      if fail_first else f"exit {apt_status}")
            for name, script in {"sudo": f'echo "$*" >> "$CALLS"\n{status}', "timeout": TIMEOUT, "sleep": SLEEP,
                                 "touch": ': > "$1"',
                                 "skopeo": skopeo, "zstd": zstd, "python3": python}.items():
                if script is not None:
                    path = root / name
                    path.write_text("#!/bin/sh\n" + script + "\n")
                    path.chmod(0o700)
            log = root / "calls"
            result = subprocess.run(["/bin/bash", str(SCRIPT)], capture_output=True,
                                    env={"CD_COMPONENT": component, "CALLS": str(log), "PATH": os.fspath(root)})
            return result.returncode, log.read_text().splitlines() if log.exists() else []

    def test_web_uses_capable_existing_binary(self):
        self.assertEqual(self.check_transport("web", 'echo "--preserve-digests"'), (0, []))

    def test_absent_incompatible_or_nonweb_uses_installer(self):
        install = (0, UPDATE + INSTALL)
        for component, binary in (("web", None), ("web", "exit 1"), ("web", "echo legacy"),
                                  ("am", 'echo "--preserve-digests"')):
            with self.subTest(component=component, binary=binary):
                self.assertEqual(self.check_transport(component, binary), install)

    def test_missing_or_incompatible_scanner_tools_require_package_install(self):
        for missing in ({"python": "exit 1"}, {"python": None}, {"zstd": "exit 1"}, {"zstd": None}):
            with self.subTest(missing=missing):
                self.assertEqual(self.check_transport("web", 'echo "--preserve-digests"', **missing),
                                 (0, UPDATE + INSTALL))

    def test_install_failure_retries_once_then_stops_publication(self):
        # A lasting failure is retried once, then stops with the named error.
        self.assertEqual(self.check_transport("web", apt_status=4), (1, UPDATE + UPDATE))

    def test_one_transient_apt_failure_is_repeated(self):
        self.assertEqual(self.check_transport("am", apt_status=100, fail_first=True), (0, UPDATE + UPDATE + INSTALL))
