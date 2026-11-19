import os
import subprocess
import tempfile
import unittest
from pathlib import Path

RUNTIME = Path(__file__).resolve().parents[2] / "api/crates/layer4-runtime"
INSTALL = RUNTIME / "image/install-debian-openssl-cli.sh"


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


if __name__ == "__main__":
    unittest.main()
