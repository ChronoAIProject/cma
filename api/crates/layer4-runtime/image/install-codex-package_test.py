import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    "install_codex_package", Path(__file__).with_name("install-codex-package.py")
)
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class InstallCodexPackageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.vendor = self.root / "vendor"
        self.target = "x86_64-unknown-linux-musl"
        self.version = "0.165.0"
        self.metadata = {
            "layoutVersion": 1,
            "version": self.version,
            "target": self.target,
            "variant": "codex",
            "entrypoint": "bin/codex",
            "resourcesDir": "codex-resources",
            "pathDir": "codex-path",
        }
        for name in package.EXECUTABLES:
            path = self.vendor / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"stock")
        (self.vendor / package.METADATA).write_text(json.dumps(self.metadata))
        self.voice = self.vendor / "codex-resources/voice/manifest.json"
        self.voice.parent.mkdir()
        self.voice.write_text("stock voice resources")
        self.archive = self.root / "codex-package.tar.gz"
        self.contents = {
            name: ("fork " + name).encode() for name in package.EXECUTABLES
        }
        self.contents[package.METADATA] = json.dumps(self.metadata).encode()
        self.expected = []
        for name in package.EXECUTABLES:
            path = self.root / "digest-input"
            path.write_bytes(self.contents[name])
            self.expected.append(package.sha256(path))

    def write_archive(self, extra=None):
        with tarfile.open(self.archive, "w:gz") as archive:
            for name, content in self.contents.items():
                info = tarfile.TarInfo(name)
                info.size = len(content)
                archive.addfile(info, io.BytesIO(content))
            if extra:
                archive.addfile(extra)

    def install(self, digest=None):
        package.install(
            self.archive, digest or package.sha256(self.archive),
            self.target, self.version, self.vendor, self.expected,
        )

    def assert_stock(self):
        for name in package.EXECUTABLES:
            self.assertEqual((self.vendor / name).read_bytes(), b"stock")
        self.assertEqual(self.voice.read_text(), "stock voice resources")

    def test_installs_all_matching_executables_and_preserves_resources(self):
        self.write_archive()
        self.install()
        for name, digest in zip(package.EXECUTABLES, self.expected):
            path = self.vendor / name
            self.assertEqual(package.sha256(path), digest)
            self.assertEqual(path.stat().st_mode & 0o777, 0o755)
        self.assertEqual(self.voice.read_text(), "stock voice resources")

    def test_refuses_incomplete_or_mismatched_packages_before_install(self):
        del self.contents["codex-resources/bwrap"]
        self.write_archive()
        with self.assertRaisesRegex(ValueError, "Incomplete"):
            self.install()
        self.assert_stock()
        self.contents["codex-resources/bwrap"] = b"wrong"
        self.write_archive()
        with self.assertRaisesRegex(ValueError, "executable digest mismatch"):
            self.install()
        self.assert_stock()

    def test_refuses_wrong_archive_or_target(self):
        self.write_archive()
        with self.assertRaisesRegex(ValueError, "archive digest mismatch"):
            self.install("0" * 64)
        self.assert_stock()
        self.contents[package.METADATA] = json.dumps({
            **self.metadata, "target": "aarch64-unknown-linux-musl",
        }).encode()
        self.write_archive()
        with self.assertRaisesRegex(ValueError, "release metadata mismatch"):
            self.install()
        self.assert_stock()

    def test_refuses_unexpected_file_types_and_paths(self):
        link = tarfile.TarInfo("bin/unexpected")
        link.type = tarfile.SYMTYPE
        link.linkname = "../../outside"
        self.write_archive(link)
        with self.assertRaisesRegex(ValueError, "Unexpected"):
            self.install()
        self.assert_stock()


if __name__ == "__main__":
    unittest.main()
