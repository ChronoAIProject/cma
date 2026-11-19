"""Verify the web runner shortcut and its package-manager fallback."""
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

INSTALL = Path(__file__).with_name("staging_cd_transport.sh")


class TransportTests(unittest.TestCase):
    def check_transport(self, component, skopeo=None, apt_status=0):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for name, script in {"sudo": f'echo "$*" >> "$CALLS"\nexit {apt_status}', "skopeo": skopeo}.items():
                if script is not None:
                    path = root / name
                    path.write_text("#!/bin/sh\n" + script + "\n")
                    path.chmod(0o700)
            log = root / "calls"
            result = subprocess.run(["/bin/bash", str(INSTALL)], capture_output=True,
                                    env={"CD_COMPONENT": component, "CALLS": str(log), "PATH": os.fspath(root)})
            return result.returncode, log.read_text().splitlines() if log.exists() else []

    def test_web_uses_capable_existing_binary(self):
        self.assertEqual(self.check_transport("web", 'echo "--preserve-digests"'), (0, []))

    def test_absent_incompatible_or_nonweb_uses_installer(self):
        install = (0, ["apt-get update -qq", "apt-get install -y -qq skopeo"])
        for component, binary in (("web", None), ("web", "exit 1"), ("web", "echo legacy"),
                                  ("am", 'echo "--preserve-digests"')):
            with self.subTest(component=component, binary=binary):
                self.assertEqual(self.check_transport(component, binary), install)

    def test_install_failure_stops_publication(self):
        self.assertEqual(self.check_transport("web", apt_status=4), (4, ["apt-get update -qq"]))
