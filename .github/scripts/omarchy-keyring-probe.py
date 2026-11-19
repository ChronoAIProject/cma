"""Exercise first-boot package trust without emitting key identifiers or contents."""

from concurrent.futures import ThreadPoolExecutor
import importlib.machinery
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SOURCE = "/usr/local/libexec/cma/initialize-sandbox"
loader = importlib.machinery.SourceFileLoader("sandbox_keyring", SOURCE)
spec = importlib.util.spec_from_loader(loader.name, loader)
KEYRING = importlib.util.module_from_spec(spec)
spec.loader.exec_module(KEYRING)


def fields(ring, *args):
    output = subprocess.check_output(["/usr/bin/gpg", "--homedir", str(ring), "--batch",
                                      "--with-colons", *args], stderr=subprocess.DEVNULL)
    return [line.split(b":") for line in output.splitlines()]


def master(ring):
    rows = fields(ring, "--list-secret-keys", KEYRING.IDENTITY)
    if sum(row[0] == b"sec" for row in rows) != 1:
        raise AssertionError("expected one sandbox master key")
    return next(row[9] for row in rows if row[0] == b"fpr")


class SandboxKeyringTests(unittest.TestCase):
    def test_supervisor_initialized_the_default_tools_and_distro_trust(self):
        self.assertEqual(os.getuid(), 10001)
        ring = KEYRING.STATE / "gnupg"
        configured = subprocess.check_output(["/usr/local/bin/pacman-conf", "GPGDir"])
        self.assertEqual(Path(os.fsdecode(configured.strip())), ring)
        subprocess.run(["/usr/local/bin/pacman-key", "--list-keys", KEYRING.IDENTITY],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        before = master(ring)
        KEYRING.initialize()
        self.assertTrue(before == master(ring), "restart replaced the key")
        for package in KEYRING.PACKAGES:
            distribution = KEYRING.DISTRIBUTION
            for key in KEYRING.records(distribution / (package + "-trusted"), r"([A-Fa-f0-9]{40}):[1-6]:"):
                rows = fields(ring, "--list-sigs", key)
                self.assertTrue(any(row[0] == b"sig" and row[4] == before[-16:] for row in rows),
                                "missing local signature on a distribution trust anchor")
            for key in KEYRING.records(distribution / (package + "-revoked"), r"([A-Fa-f0-9]{40})"):
                rows = fields(ring, "--list-keys", key)
                self.assertTrue(any(row[0] == b"pub" and b"D" in row[11] for row in rows),
                                "revoked distribution key remains enabled")

    def test_failed_first_boot_recovers_with_a_different_key(self):
        with tempfile.TemporaryDirectory(dir="/dev/shm") as directory:
            state = Path(directory)
            with self.assertRaises(KEYRING.KeyringError):
                KEYRING.initialize(state, state / "missing-inputs")
            self.assertEqual(list(state.iterdir()), [])
            # A second process may start while the first is still generating
            # its key. Each call takes its own flock on the state directory.
            (state / "initializing").mkdir(mode=0o700)
            with ThreadPoolExecutor(max_workers=2) as pool:
                list(pool.map(KEYRING.initialize, (state, state)))
            ring = state / "gnupg"
            try:
                self.assertTrue(master(ring) != master(KEYRING.STATE / "gnupg"), "sandbox key reused")
                (ring / "trustdb.gpg").unlink()
                with self.assertRaisesRegex(KEYRING.KeyringError, "incomplete"):
                    KEYRING.initialize(state)
            finally:
                subprocess.run(["/usr/bin/gpgconf", "--homedir", str(ring), "--kill", "all"], check=True)

    def test_unsafe_or_indirect_state_is_refused(self):
        with tempfile.TemporaryDirectory(dir="/dev/shm") as directory:
            root = Path(directory)
            alias = root / "alias"
            alias.symlink_to(root)
            with self.assertRaises(OSError):
                KEYRING.initialize(alias)
            (root / "gnupg").symlink_to(root)
            with self.assertRaisesRegex(KEYRING.KeyringError, "invalid"):
                KEYRING.initialize(root)
            invalid = root / "invalid-trust"
            invalid.write_text("# fixture comment\n\ninvalid\n")
            with self.assertRaisesRegex(KEYRING.KeyringError, "record"):
                list(KEYRING.records(invalid, r"([A-F0-9]{40}):[1-6]:"))
            root.chmod(0o755)
            with self.assertRaisesRegex(KEYRING.KeyringError, "unsafe"):
                KEYRING.initialize(root)


if __name__ == "__main__":
    unittest.main()
