"""Distribution stripping rejects content or package drift without deleting it."""
import hashlib
import tempfile
import unittest
from pathlib import Path

from chromium_distribution import strip


class ChromiumDistributionTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.data = b"synthetic distribution configuration\n"
        self.expected = {
            "package": {"name": "chromium", "version": "1", "architecture": "amd64"},
            "file": {"path": "etc/chromium.d/apikeys", "bytes": len(self.data),
                     "sha256": hashlib.sha256(self.data).hexdigest()},
        }
        self.installed = "chromium\n1\namd64\n"
        self.target = self.root / self.expected["file"]["path"]
        self.target.parent.mkdir(parents=True)
        self.target.write_bytes(self.data)

    def test_exact_file_removed_with_browser_and_other_configuration_preserved(self):
        keep = self.target.with_name("default-flags")
        keep.write_bytes(b"synthetic browser flags")
        browser = self.root / "usr/bin/chromium"
        browser.parent.mkdir(parents=True)
        browser.write_bytes(b"synthetic browser")
        strip(self.root, self.expected, self.installed)
        self.assertFalse(self.target.exists())
        self.assertEqual(keep.read_bytes(), b"synthetic browser flags")
        self.assertEqual(browser.read_bytes(), b"synthetic browser")

    def test_modified_file_refuses_even_at_the_same_length(self):
        for data in (b"x" + self.data[1:], self.data + b"extra"):
            with self.subTest(length=len(data)):
                self.target.write_bytes(data)
                with self.assertRaises(ValueError):
                    strip(self.root, self.expected, self.installed)
                self.assertEqual(self.target.read_bytes(), data)

    def test_package_name_version_architecture_or_extra_record_refuses(self):
        for installed in ("other\n1\namd64\n", "chromium\n2\namd64\n",
                          "chromium\n1\narm64\n", self.installed + self.installed):
            with self.subTest(installed=installed), self.assertRaises(ValueError):
                strip(self.root, self.expected, installed)
            self.assertTrue(self.target.is_file())

    def test_missing_file_or_symlink_refuses_without_changing_destination(self):
        self.target.unlink()
        with self.assertRaises(ValueError):
            strip(self.root, self.expected, self.installed)
        other = self.root / "preserve"
        other.write_bytes(self.data)
        self.target.symlink_to(other)
        with self.assertRaises(ValueError):
            strip(self.root, self.expected, self.installed)
        self.assertEqual(other.read_bytes(), self.data)
        self.target.unlink()
        self.target.parent.rmdir()
        self.target.parent.symlink_to(self.root)
        with self.assertRaises(ValueError):
            strip(self.root, self.expected, self.installed)


if __name__ == "__main__":
    unittest.main()
