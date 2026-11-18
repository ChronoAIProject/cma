"""Deterministic packaging preserves package identity and public trust inputs."""

import importlib.util
from pathlib import Path
import tempfile
import unittest


SOURCE = Path(__file__).resolve().parents[2] / "api/crates/layer4-runtime/image/normalize-omarchy.py"
SPEC = importlib.util.spec_from_file_location("normalize_omarchy", SOURCE)
NORMALIZE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(NORMALIZE)
PATH_SPEC = importlib.util.spec_from_file_location("login_path", SOURCE.with_name("login-path.py"))
LOGIN = importlib.util.module_from_spec(PATH_SPEC)
PATH_SPEC.loader.exec_module(LOGIN)


class MetadataTests(unittest.TestCase):
    def test_login_profile_preserves_supplied_path_and_rejects_distro_drift(self):
        original = '\n'.join([
            'if [ "$(id -u)" -eq 0 ]; then',
            '  PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"',
            'else', '  PATH="/usr/local/bin:/usr/bin:/bin:/usr/local/games:/usr/games"',
            'fi', 'export PATH',
        ])
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "profile"
            path.write_text(original)
            LOGIN.rewrite(path)
            self.assertTrue(path.read_text().startswith('if [ -z "${PATH:-}" ]; then\n'))
            self.assertIn('$(/usr/bin/id -u)', path.read_text())
            self.assertTrue(path.read_text().endswith('fi\nexport PATH'))
            for drift in ("changed upstream profile", original + '\n' + original):
                path.write_text(drift)
                with self.assertRaises(SystemExit):
                    LOGIN.rewrite(path)
                self.assertEqual(path.read_text(), drift)

    def test_only_install_time_and_build_local_material_are_removed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            desc = root / "var/lib/pacman/local/example-1.0/desc"
            desc.parent.mkdir(parents=True)
            desc.write_text("%NAME%\nexample\n\n%VERSION%\n1.0\n\n%BUILDDATE%\n123\n\n%INSTALLDATE%\n456\n")
            public = root / "usr/share/pacman/keyrings/example.gpg"
            public.parent.mkdir(parents=True)
            public.write_bytes(b"synthetic public distribution keyring")
            generated = root / "etc/pacman.d/gnupg/private-keys-v1.d/generated-fixture"
            generated.parent.mkdir(parents=True)
            generated.write_bytes(b"synthetic build-local state")
            trust = generated.parent.parent / "trustdb.gpg"
            trust.write_bytes(b"synthetic public trust database")
            pubring = trust.with_name("pubring.gpg")
            pubring.write_bytes(b"synthetic installed public keyring")
            entropy = trust.with_name("random_seed")
            entropy.write_bytes(b"synthetic local entropy")
            log = root / "var/log/pacman.log"
            log.parent.mkdir(parents=True)
            log.write_text("time-dependent installer output")
            NORMALIZE.normalize(root)
            self.assertIn("%VERSION%\n1.0\n", desc.read_text())
            self.assertIn("%BUILDDATE%\n123\n", desc.read_text())
            self.assertIn("%INSTALLDATE%\n0\n", desc.read_text())
            self.assertEqual(public.read_bytes(), b"synthetic public distribution keyring")
            self.assertFalse(generated.parent.exists())
            self.assertEqual(trust.read_bytes(), b"synthetic public trust database")
            self.assertEqual(pubring.read_bytes(), b"synthetic installed public keyring")
            self.assertFalse(entropy.exists())
            self.assertFalse(log.exists())
            before = desc.read_bytes()
            NORMALIZE.normalize(root)
            self.assertEqual(desc.read_bytes(), before)

    def test_unknown_package_metadata_refuses_before_keyring_cleanup(self):
        for value in ("%NAME%\nexample", "%INSTALLDATE%\nbad", "%INSTALLDATE%\n1\n%INSTALLDATE%\n2"):
            with self.subTest(value=value), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                desc = root / "var/lib/pacman/local/example/desc"
                desc.parent.mkdir(parents=True)
                desc.write_text(value)
                generated = root / "etc/pacman.d/gnupg"
                generated.mkdir(parents=True)
                with self.assertRaises(ValueError):
                    NORMALIZE.normalize(root)
                self.assertTrue(generated.exists())
                self.assertEqual(desc.read_text(), value)


if __name__ == "__main__":
    unittest.main()
