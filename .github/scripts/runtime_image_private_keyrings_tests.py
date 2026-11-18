"""Private GnuPG storage refuses image construction independently of scanning."""
import contextlib
import importlib.util
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


SOURCE = Path(__file__).resolve().parents[2] / "api/crates/layer4-runtime/image/assert-no-private-keyrings.py"
SPEC = importlib.util.spec_from_file_location("private_keyring_gate", SOURCE)
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)


class PrivateKeyringTests(unittest.TestCase):
    def test_private_files_and_secret_ring_backups_refuse_in_any_image_home(self):
        paths = ("etc/pacman.d/gnupg/private-keys-v1.d/synthetic.key",
                 "home/cma/.gnupg/private-keys-v1.d/nested/synthetic",
                 "root/.gnupg/secring.gpg", "opt/tool/secring.gpg~",
                 "var/lib/custom-keyring/secring.gpg.backup", "tmp/secring")
        for name in paths:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                item = root / name
                item.parent.mkdir(parents=True)
                item.write_bytes(b"synthetic non-secret fixture")
                self.assertEqual(GATE.private_keyrings(root), [name])
                with contextlib.redirect_stdout(io.StringIO()) as output:
                    self.assertEqual(GATE.main(root), 1)
                self.assertEqual(output.getvalue(),
                                 "Runtime private-keyring check failed: private_entries=1\n")

    def test_public_keys_trust_databases_and_empty_private_directory_pass(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("etc/pacman.d/gnupg/pubring.gpg", "etc/pacman.d/gnupg/pubring.kbx",
                         "etc/pacman.d/gnupg/trustdb.gpg", "usr/share/pacman/keyrings/archlinux.gpg"):
                item = root / name
                item.parent.mkdir(parents=True, exist_ok=True)
                item.write_bytes(b"synthetic public fixture")
            (root / "etc/pacman.d/gnupg/private-keys-v1.d").mkdir()
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(GATE.main(root), 0)

    def test_symlinks_cannot_hide_private_storage(self):
        for target in ("../absent", "../public"):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / "public").mkdir()
                keys = root / "gnupg/private-keys-v1.d"
                keys.parent.mkdir()
                keys.symlink_to(target, target_is_directory=True)
                self.assertEqual(GATE.private_keyrings(root), ["gnupg/private-keys-v1.d"])
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            key = root / "private-keys-v1.d/empty.key"
            key.parent.mkdir()
            key.symlink_to("missing")
            self.assertEqual(GATE.private_keyrings(root), ["private-keys-v1.d/empty.key"])

    def test_inventory_errors_fail_closed_without_disclosing_exception_values(self):
        with tempfile.TemporaryDirectory() as directory:
            def unreadable(*_args, **kwargs):
                kwargs["onerror"](PermissionError("private diagnostic value"))
            with patch.object(GATE.os, "walk", side_effect=unreadable), \
                    contextlib.redirect_stdout(io.StringIO()) as output:
                self.assertEqual(GATE.main(directory), 2)
            self.assertEqual(output.getvalue(),
                             "Runtime private-keyring check failed: incomplete_inventory\n")

    def test_missing_or_symlinked_root_refuses(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            link = root / "alias"
            link.symlink_to(root, target_is_directory=True)
            for path in (root / "missing", link):
                with self.subTest(path=path), contextlib.redirect_stdout(io.StringIO()):
                    self.assertEqual(GATE.main(path), 2)


if __name__ == "__main__":
    unittest.main()
