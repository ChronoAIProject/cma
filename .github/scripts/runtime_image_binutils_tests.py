"""The runtime retains Debian's legal notices across package installation and refresh."""
import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


RUNTIME = Path(__file__).resolve().parents[2] / "api/crates/layer4-runtime"
SPEC = importlib.util.spec_from_file_location("notices", RUNTIME / "image/normalize-binutils-notices.py")
NOTICES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(NOTICES)


class BinutilsNoticeTests(unittest.TestCase):
    def test_only_reviewed_line_changes_in_all_installed_copies(self):
        locator = b"  cvs://example.invalid/project\n"
        before, after = b"Source follows.\n\n", b"\nCopyright and licensing.\n"
        original = before + locator + after
        with tempfile.TemporaryDirectory() as directory, patch.multiple(
            NOTICES, NOTICE_SHA256=hashlib.sha256(original).hexdigest(),
            LEGACY_LINE_SHA256=hashlib.sha256(locator).hexdigest(),
        ):
            root = Path(directory)
            copies = [root / "usr/share/doc" / package / "copyright" for package in NOTICES.PACKAGES]
            for path in copies:
                path.parent.mkdir(parents=True)
                path.write_bytes(original)
            self.assertEqual(NOTICES.normalize(root), 4)
            for path in copies:
                self.assertEqual(path.read_bytes(), before + NOTICES.SOURCE_LOCATION + after)
            self.assertEqual(NOTICES.normalize(root), 0)

    def test_absent_or_updated_package_and_unreviewed_locator(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.assertEqual(NOTICES.normalize(root), 0)
            path = root / "usr/share/doc/binutils/copyright"
            path.parent.mkdir(parents=True)
            for original in (b"Upstream has updated these source instructions.\n", b"cvs://example.invalid/other\n"):
                path.write_bytes(original)
                if b"cvs://" in original:
                    with self.assertRaisesRegex(ValueError, "Unrecognized"):
                        NOTICES.normalize(root)
                else:
                    self.assertEqual(NOTICES.normalize(root), 0)
                self.assertEqual(path.read_bytes(), original)
            path.unlink()
            outside = root / "another-notice"
            outside.write_bytes(b"untouched")
            path.symlink_to(outside)
            with self.assertRaisesRegex(ValueError, "target"):
                NOTICES.normalize(root)
            self.assertEqual(outside.read_bytes(), b"untouched")

    def test_both_package_installations_correct_the_notice_in_the_same_layer(self):
        instructions = (RUNTIME / "Dockerfile").read_text().replace("\\\n", "").splitlines()
        for package_step in ("sh /tmp/install-os.sh", "apt-get upgrade -y"):
            with self.subTest(package_step=package_step):
                [run] = [line for line in instructions if line.startswith("RUN ") and package_step in line
                         and "normalize-machine-id.py" in line]
                self.assertIn("source=api/crates/layer4-runtime/image/normalize-binutils-notices.py,", run)
                self.assertIn("python3 -B /tmp/normalize-binutils-notices.py", run.split(package_step, 1)[1])


if __name__ == "__main__":
    unittest.main()
