"""Private signing state must never enter the copied Omarchy image layers."""
import os
import socket
import tempfile
import unittest
from pathlib import Path

from pacman_keyring import strip_private_keyring


class PacmanKeyringTests(unittest.TestCase):
    def make_keyring(self, root):
        keyring = root / "etc/pacman.d/gnupg"
        keyring.mkdir(parents=True)
        for name in ("pubring.gpg", "trustdb.gpg", "gpg.conf", "tofu.db"):
            (keyring / name).write_bytes(b"synthetic retained verification state")
        for name in ("private-keys-v1.d", "openpgp-revocs.d"):
            (keyring / name).mkdir(mode=0o700)
            (keyring / name / "synthetic").write_bytes(b"synthetic disposable state")
        return keyring

    def test_removes_private_state_backups_and_sockets_preserving_public_state(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            keyring = self.make_keyring(root)
            before = {p.name: (p.read_bytes(), p.stat().st_mode)
                      for p in keyring.iterdir() if p.is_file()}
            for name in ("secring.gpg", "pubring.gpg~", "private-keys-v1.d/cache~"):
                (keyring / name).write_bytes(b"synthetic disposable state")
            external = root / "retained"
            external.write_bytes(b"unrelated bytes")
            os.symlink(external, keyring / "external~")
            # Bind under a short temporary path to respect Unix socket limits.
            with socket.socket(socket.AF_UNIX) as endpoint:
                endpoint.bind(str(keyring / "S.agent"))
                self.assertGreater(strip_private_keyring(root), 0)
            self.assertEqual(external.read_bytes(), b"unrelated bytes")
            for name, expected in before.items():
                self.assertEqual(((keyring / name).read_bytes(), (keyring / name).stat().st_mode),
                                 expected)
            for name in ("private-keys-v1.d", "openpgp-revocs.d"):
                self.assertEqual(list((keyring / name).iterdir()), [])
                self.assertEqual((keyring / name).stat().st_mode & 0o777, 0o700)
            self.assertFalse(any(p.name.endswith("~") or p.name.startswith("S.")
                                 for p in keyring.rglob("*")))
            self.assertFalse((keyring / "secring.gpg").exists())
            self.assertEqual(strip_private_keyring(root), 0)

    def test_invalid_public_state_refuses_before_stripping(self):
        for name in ("pubring.gpg", "trustdb.gpg"):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                keyring = self.make_keyring(root)
                (keyring / name).unlink()
                with self.assertRaises(ValueError):
                    strip_private_keyring(root)
                self.assertTrue((keyring / "private-keys-v1.d/synthetic").is_file())

    def test_redirected_private_directory_refuses_before_stripping(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            keyring = self.make_keyring(root)
            (keyring / "openpgp-revocs.d/synthetic").unlink()
            (keyring / "openpgp-revocs.d").rmdir()
            os.symlink(root, keyring / "openpgp-revocs.d")
            with self.assertRaises(ValueError):
                strip_private_keyring(root)
            self.assertTrue((keyring / "private-keys-v1.d/synthetic").is_file())

    def test_redirected_keyring_ancestor_refuses(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            keyring = self.make_keyring(root)
            original = root / "original"
            (root / "etc").rename(original)
            os.symlink(original, root / "etc")
            with self.assertRaises(ValueError):
                strip_private_keyring(root)
            self.assertTrue((keyring / "private-keys-v1.d/synthetic").exists())

    def test_cleanup_is_only_in_omarchy_prepared_stage(self):
        recipe = Path(__file__).resolve().parents[1].joinpath("Dockerfile").read_text()
        prepared = recipe.split("FROM omarchy-dependencies AS omarchy-prepared\n", 1)[1]
        prepared = prepared.split("FROM scratch AS omarchy\n", 1)[0]
        self.assertEqual(recipe.count("pacman_keyring.py --root /"), 1)
        self.assertIn("pacman_keyring.py --root /", prepared)


if __name__ == "__main__":
    unittest.main()
