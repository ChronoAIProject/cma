"""Deterministic packaging preserves package identity and public trust inputs."""

import hashlib
import importlib.util
import subprocess
import tarfile
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path
from unittest.mock import patch

SOURCE = Path(__file__).resolve().parents[2] / "api/crates/layer4-runtime/image/normalize-omarchy.py"
SPEC = importlib.util.spec_from_file_location("normalize_omarchy", SOURCE)
NORMALIZE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(NORMALIZE)
PATH_SPEC = importlib.util.spec_from_file_location("login_path", SOURCE.with_name("login-path.py"))
LOGIN = importlib.util.module_from_spec(PATH_SPEC)
PATH_SPEC.loader.exec_module(LOGIN)


class MetadataTests(unittest.TestCase):
    def test_standard_runtime_ships_the_stale_turn_seal_enabled(self):
        recipe = (SOURCE.parent.parent / "Dockerfile").read_text()
        standard = recipe.split("FROM runtime-refreshed AS runtime\n", 1)[1].split("\nFROM ", 1)[0]
        environment = [line.split()[1:] for line in standard.replace("\\\n", "").splitlines()
                       if line.startswith("ENV ")]
        assignments = [item for statement in environment for item in statement
                       if item.startswith("CHRONO_RUNTIME_STALE_TURN_SEAL_ENABLED=")]
        self.assertEqual(assignments, ["CHRONO_RUNTIME_STALE_TURN_SEAL_ENABLED=1"])

    def test_omarchy_cleanup_ships_in_the_package_producer_layer(self):
        runtime = SOURCE.parent.parent
        installer = (runtime / "src/omarchy/install.sh").read_text().splitlines()
        cleanup = "python /tmp/normalize-omarchy.py"
        self.assertEqual(installer.count(cleanup), 1)
        self.assertGreater(installer.index(cleanup), next(i for i, line in enumerate(installer)
                                                          if line.startswith("pacman -Q > ")))
        recipe = (runtime / "Dockerfile").read_text().split(" AS omarchy-dependencies\n", 1)[1]
        producer = recipe.split("\nFROM ", 1)[0].splitlines()
        install = next(i for i, line in enumerate(producer)
                       if line.startswith("RUN ") and "bash /tmp/install-omarchy.sh" in line)
        self.assertIn(f"COPY {SOURCE.relative_to(runtime.parents[2]).as_posix()} /tmp/normalize-omarchy.py",
                      producer[:install])

    def test_omarchy_package_install_layer_excludes_optional_files(self):
        suites = ("usr/share/omarchy/test/shell.d/git-url-check-test.sh",
                  "usr/lib/python3.14/site-packages/numpy/random/tests/test_generator_mt19937.py")
        manuals = ("usr/share/doc/zmq/zmq_curve.html", "usr/share/man/man7/zmq_curve.7.gz",
                   "usr/share/man/man3/HTTP::Tiny.3perl.gz", "usr/share/man/man5/foot.ini.5.gz",
                   "usr/share/man/man1/git-credential-store.1.gz")
        sources = ("usr/share/git/credential/libsecret/Makefile",
                   "usr/share/git/credential/osxkeychain/git-credential-osxkeychain.c",
                   "usr/share/perl5/core_perl/pod/perlguts.pod")
        optional = set(suites + manuals + sources)
        for name in ("ffmpeg-all", "ffmpeg-protocols", "ffplay-all", "ffprobe-all"):
            optional.update((f"usr/share/doc/ffmpeg/{name}.html", f"usr/share/man/man1/{name}.1.gz"))
        optional.update(("usr/share/omarchy/test/shell.d/retired-installer-artifacts-migration-test.sh",
                         "var/cache/pacman/pkg/.partial", "var/cache/pacman/pkg/package.pkg.tar.zst",
                         "root/.cache/pip/http-v2/response"))
        runtime = {"usr/share/omarchy/bin/omarchy-launch-browser", "usr/bin/ffmpeg",
                   "usr/lib/python3.14/site-packages/numpy/random/__init__.py",
                   "usr/lib/python3.14/site-packages/numpy/testing/__init__.py",
                   "usr/share/man/man1/git.1.gz", "usr/share/licenses/ffmpeg/COPYING",
                   "usr/lib/git-core/git-credential-store", "usr/bin/perl",
                   "usr/share/doc/ffmpeg/LICENSE", "usr/share/pacman/keyrings/archlinux.gpg"}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "root"
            for name in optional | runtime:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b"synthetic package payload")
            # Package archives may link one manual to another. Dropping the
            # optional entry must preserve the retained link target.
            manual = root / "usr/share/man/man1/git-credential-store.1.gz"
            manual.unlink()
            manual.symlink_to("git.1.gz")
            NORMALIZE.normalize(root)
            with tarfile.open(Path(directory) / "package-layer.tar", "w:") as layer:
                layer.add(root, arcname=".")
            with tarfile.open(Path(directory) / "package-layer.tar") as layer:
                entries = {str(Path(member.name)) for member in layer}
            self.assertFalse(optional & entries)
            self.assertTrue(runtime <= entries)
            self.assertIn("var/cache/pacman/pkg", entries)

    def test_portal_cleanup_preserves_license_and_dbus_signatures(self):
        description = b"<!-- Fixture description -->"
        retained = b'<!-- Fixture license --><node><interface><method name="Read"/></interface></node>'
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            directory = root / "usr/share/dbus-1/interfaces"
            directory.mkdir(parents=True)
            digest = hashlib.sha256(description).hexdigest()
            inputs = tuple((name, digest) for name, _ in NORMALIZE.PORTAL_DESCRIPTIONS)
            for name, _ in inputs:
                (directory / name).write_bytes(description + retained)
            with patch.object(NORMALIZE, "PORTAL_DESCRIPTIONS", inputs):
                NORMALIZE.remove_portal_descriptions(root)
            for name, _ in inputs:
                updated = (directory / name).read_bytes()
                self.assertEqual(updated, retained)
                self.assertEqual(ET.tostring(ET.fromstring(updated)),
                                 ET.tostring(ET.fromstring(description + retained)))

    def test_portal_cleanup_refuses_an_unreviewed_description(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = root / "usr/share/dbus-1/interfaces/org.freedesktop.portal.Secret.xml"
            path.parent.mkdir(parents=True)
            original = b"<!-- Unreviewed fixture description --><node/>"
            path.write_bytes(original)
            with self.assertRaisesRegex(ValueError, "Review changed D-Bus portal description"):
                NORMALIZE.remove_portal_descriptions(root)
            self.assertEqual(path.read_bytes(), original)

    def test_omarchy_omits_the_standalone_cli_and_keeps_the_package_libraries(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cli = root / "usr/bin/gnutls-cli"
            library = root / "usr/lib/libgnutls.so.30"
            package = root / "var/lib/pacman/local/gnutls-fixture/files"
            for path in (cli, library, package):
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b"fixture")
            listing = subprocess.CompletedProcess([], 0, stdout=f"gnutls {cli.resolve()}\n")
            with patch.object(NORMALIZE.subprocess, "run", return_value=listing) as runner:
                NORMALIZE.remove_optional_gnutls_cli(root)
                NORMALIZE.remove_optional_gnutls_cli(root)
            runner.assert_called_once_with(["pacman", "--root", str(root.resolve()), "-Ql", "gnutls"],
                                           capture_output=True, text=True, check=True, timeout=20)
            self.assertFalse(cli.exists())
            self.assertEqual(library.read_bytes(), b"fixture")
            self.assertEqual(package.read_bytes(), b"fixture")

    def test_omarchy_refuses_to_remove_an_unowned_or_indirect_command(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cli = root / "usr/bin/gnutls-cli"
            cli.parent.mkdir(parents=True)
            cli.write_bytes(b"fixture")
            listing = subprocess.CompletedProcess([], 0, stdout="gnutls /usr/lib/libgnutls.so.30\n")
            with (
                patch.object(NORMALIZE.subprocess, "run", return_value=listing),
                self.assertRaisesRegex(ValueError, "does not own"),
            ):
                NORMALIZE.remove_optional_gnutls_cli(root)
            self.assertEqual(cli.read_bytes(), b"fixture")
            cli.unlink()
            cli.symlink_to("other")
            with self.assertRaisesRegex(ValueError, "Unexpected GnuTLS command target"):
                NORMALIZE.remove_optional_gnutls_cli(root)
            self.assertTrue(cli.is_symlink())

    def test_package_installs_use_nonzero_epoch_before_normalization(self):
        installer = SOURCE.parent.parent / "src/omarchy/install.sh"
        commands = [line.rstrip(" \\") for line in installer.read_text().splitlines()
                    if "pacman -U " in line or "pacman -Syu " in line]
        self.assertEqual(len(commands), 2)
        # Exercise the actual shell prefixes with a package-manager substitute.
        # Epoch zero causes libalpm to omit the field our normalizer validates.
        for command in commands:
            result = subprocess.run(
                ["bash", "-c", "export SOURCE_DATE_EPOCH=0; "
                 "pacman() { test \"$SOURCE_DATE_EPOCH\" = 1; }; "
                 + command + "; result=$?; test $result = 0 && "
                 "test \"$SOURCE_DATE_EPOCH\" = 0"],
                check=False, capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_login_profile_preserves_supplied_path_and_rejects_distro_drift(self):
        original = (
            'if [ "$(id -u)" -eq 0 ]; then\n'
            '  PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"\n'
            'else\n  PATH="/usr/local/bin:/usr/bin:/bin:/usr/local/games:/usr/games"\n'
            'fi\nexport PATH'
        )
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
            legacy = [trust.with_name(name) for name in ("secring.gpg", "secring.gpg~", "secring")]
            for path in legacy:
                path.write_bytes(b"synthetic legacy private storage")
            log = root / "var/log/pacman.log"
            log.parent.mkdir(parents=True)
            log.write_text("time-dependent installer output")
            sync = root / "var/lib/pacman/sync"
            sync.mkdir()
            for name in ("core.db", "extra.db", "multilib.db", "omarchy.db", "core.db.sig"):
                (sync / name).write_bytes(b"synthetic repository index")
            NORMALIZE.normalize(root)
            self.assertIn("%VERSION%\n1.0\n", desc.read_text())
            self.assertIn("%BUILDDATE%\n123\n", desc.read_text())
            self.assertIn("%INSTALLDATE%\n0\n", desc.read_text())
            self.assertEqual(public.read_bytes(), b"synthetic public distribution keyring")
            self.assertFalse(generated.parent.exists())
            self.assertFalse(trust.exists())
            self.assertFalse(pubring.exists())
            self.assertFalse(entropy.exists())
            self.assertTrue(all(not path.exists() for path in legacy))
            self.assertFalse(log.exists())
            self.assertTrue(sync.is_dir())
            self.assertEqual(list(sync.iterdir()), [])
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

    def test_documentation_examples_leave_only_reviewed_lines(self):
        retained = b"first retained line\n"
        example = b"reviewed example line\n"
        original = retained + example + b"second retained line\n" + example
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = root / "usr/share/doc/fixture.txt"
            path.parent.mkdir(parents=True)
            path.write_bytes(original)
            reviewed = (("usr/share/doc/fixture.txt", hashlib.sha256(original).hexdigest(),
                         (hashlib.sha256(example).hexdigest(),), 2),)
            with patch.object(NORMALIZE, "DOCUMENTATION_EXAMPLES", reviewed):
                NORMALIZE.remove_documentation_examples(root)
                self.assertEqual(path.read_bytes(), retained + b"second retained line\n")
                NORMALIZE.remove_documentation_examples(root)
                self.assertEqual(path.read_bytes(), retained + b"second retained line\n")
                # A changed upstream file carrying the example needs a new review.
                changed = original + b"unreviewed upstream addition\n"
                path.write_bytes(changed)
                with self.assertRaisesRegex(ValueError, "Documentation example source requires review"):
                    NORMALIZE.remove_documentation_examples(root)
                self.assertEqual(path.read_bytes(), changed)

    def test_bundled_wheel_remains_the_signed_package_member(self):
        # Rewriting the wheel would leave a file its pacman mtree record no longer describes.
        stock = b"PK\x05\x06" + bytes(18) + b"synthetic package-owned wheel bytes"
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            wheel = root / NORMALIZE.ENSUREPIP_WHEEL
            wheel.parent.mkdir(parents=True)
            wheel.write_bytes(stock)
            NORMALIZE.normalize(root)
            self.assertEqual(wheel.read_bytes(), stock)

    def test_only_an_empty_object_signing_bundle_is_omitted(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bundle = root / NORMALIZE.OBJECT_SIGNING_BUNDLE
            tls = bundle.with_name("tls-ca-bundle.pem")
            bundle.parent.mkdir(parents=True)
            bundle.write_bytes(b"")
            tls.write_bytes(b"synthetic public trust bundle")
            NORMALIZE.remove_empty_object_signing_bundle(root)
            self.assertFalse(bundle.exists())
            self.assertEqual(tls.read_bytes(), b"synthetic public trust bundle")
            NORMALIZE.remove_empty_object_signing_bundle(root)
            bundle.write_bytes(b"synthetic object-signing anchor")
            with self.assertRaisesRegex(ValueError, "not empty"):
                NORMALIZE.remove_empty_object_signing_bundle(root)
            self.assertTrue(bundle.exists())


if __name__ == "__main__":
    unittest.main()
