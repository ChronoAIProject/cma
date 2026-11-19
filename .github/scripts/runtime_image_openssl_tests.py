import os
import subprocess
import tempfile
import unittest
from pathlib import Path

RUNTIME = Path(__file__).resolve().parents[2] / "api/crates/layer4-runtime"
INSTALL = RUNTIME / "image/install-debian-openssl-cli.sh"
ARCH_INSTALL = RUNTIME / "image/install-arch-openssl-cli.sh"


class OpenSSLTests(unittest.TestCase):
    def test_installs_only_a_matching_distribution_build(self):
        for rebuilt, installed, accepted in (
            ("3.5.7-1", "3.5.7-1", True),
            ("3.5.7-1", "3.5.7-2", False),
            ("", "3.5.7-1", False),
        ):
            with self.subTest(rebuilt=rebuilt, installed=installed), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                original = root / "system-openssl"
                original.write_text("distribution fixture\n")
                (root / "version").write_text(rebuilt + "\n")
                (root / "openssl").write_text("#!/bin/sh\ntest \"$1\" = version && echo rebuilt\n")
                query = root / "dpkg-query"
                query.write_text('#!/bin/sh\nprintf "%s" "$TEST_INSTALLED_VERSION"\n')
                query.chmod(0o755)
                result = subprocess.run(
                    ["sh", INSTALL, root, original],
                    env=dict(os.environ, PATH=f"{root}:{os.environ['PATH']}", TEST_INSTALLED_VERSION=installed),
                    capture_output=True, text=True, check=False,
                )
                if accepted:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(result.stdout, "rebuilt\n")
                    self.assertEqual(original.stat().st_mode & 0o777, 0o755)
                else:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("package version mismatch", result.stderr)
                    self.assertEqual(original.read_text(), "distribution fixture\n")

    def test_applies_replacement_in_each_shipped_debian_package_layer(self):
        recipe = (RUNTIME / "Dockerfile").read_text()
        for stage, action in (("runtime-system", "sh /tmp/install-os.sh"), ("runtime-refreshed", "apt-get upgrade -y")):
            with self.subTest(stage=stage):
                body = recipe.split(f" AS {stage}\n", 1)[1].split("\nFROM ", 1)[0]
                commands = [line for line in body.replace("\\\n", " ").splitlines()
                            if line.startswith("RUN ") and action in line]
                self.assertEqual(len(commands), 1)
                step = commands[0]
                self.assertIn("--mount=type=bind,from=openssl-cli-build,source=/out", step)
                self.assertGreater(step.index("sh /tmp/install-openssl-cli.sh /tmp/cma-openssl /usr/bin/openssl"),
                                   step.index(action))

    def test_arch_replaces_the_cli_only_when_package_version_matches(self):
        for installed in ("3.6.5-1", "3.6.5-2"):
            with self.subTest(installed=installed), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                original = root / "usr/bin/openssl"
                original.parent.mkdir(parents=True)
                original.write_bytes(b"distribution CLI")
                library = root / "usr/lib/libssl.so.3"
                library.parent.mkdir()
                library.write_bytes(b"distribution library")
                (root / "version").write_text("3.6.5-1\n")
                (root / "openssl").write_bytes(b"rebuilt CLI")
                pacman = root / "pacman"
                pacman.write_text('#!/bin/sh\nprintf "openssl %s\\n" "$TEST_INSTALLED_VERSION"\n')
                pacman.chmod(0o755)
                result = subprocess.run(
                    ["sh", ARCH_INSTALL, root, root],
                    env=dict(os.environ, PATH=f"{root}:{os.environ['PATH']}", TEST_INSTALLED_VERSION=installed),
                    capture_output=True, text=True, check=False,
                )
                self.assertEqual(library.read_bytes(), b"distribution library")
                if installed == "3.6.5-1":
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(original.read_bytes(), b"rebuilt CLI")
                else:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("package version mismatch", result.stderr)
                    self.assertEqual(original.read_bytes(), b"distribution CLI")

    def test_arch_replaces_the_cli_before_each_package_layer_commits(self):
        bootstrap = (RUNTIME / "image/build-omarchy-base.sh").read_text()
        replacement = 'sh /tmp/install-arch-openssl-cli.sh /arch-openssl-cli "$payload"'
        self.assertLess(bootstrap.index("--noconfirm base"), bootstrap.index(replacement))
        self.assertLess(bootstrap.index(replacement), bootstrap.index("/tmp/normalize-omarchy.py"))
        recipe = (RUNTIME / "Dockerfile").read_text()
        desktop = recipe.split(" AS omarchy-dependencies\n", 1)[1].split("\nFROM ", 1)[0]
        producer, = (line for line in desktop.splitlines() if line.startswith("RUN "))
        self.assertIn("from=omarchy-bootstrap,source=/arch-openssl-cli", producer)
        replacement = "sh /tmp/install-arch-openssl-cli.sh /run/cma-openssl-cli /"
        self.assertLess(producer.index("bash /tmp/install-omarchy.sh"), producer.index(replacement))


if __name__ == "__main__":
    unittest.main()
