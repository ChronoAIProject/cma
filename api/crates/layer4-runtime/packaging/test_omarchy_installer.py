"""Exact installer removal must preserve other menu and refresh behavior."""
import copy
import tempfile
import unittest
from pathlib import Path

from documentation import sha
from omarchy_installer import prepare


class OmarchyInstallerTests(unittest.TestCase):
    def fixture(self, root):
        removed = "usr/share/omarchy/bin/optional-installer"
        source = b"synthetic optional installer"
        (root / removed).parent.mkdir(parents=True)
        (root / removed).write_bytes(source)
        policy = {"schema": "omarchy-optional-installer-strips-v1",
                  "remove": [{"path": removed, "bytes": len(source), "source_sha256": sha(source)}],
                  "edit": []}
        for name in ("refresh", "menu"):
            path = "usr/share/omarchy/bin/" + name
            original, result = b"keep\nremove\nkeep\n", b"keep\nkeep\n"
            (root / path).write_bytes(original)
            (root / path).chmod(0o755)
            policy["edit"].append({"path": path, "bytes": len(original),
                                   "source_sha256": sha(original), "result_sha256": sha(result),
                                   "remove": [{"start": 5, "end": 12, "sha256": sha(b"remove\n")}]})
        return policy

    def test_exact_removal_preserves_caller_bytes_and_modes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            policy = self.fixture(root)
            prepare(root, policy)
            self.assertFalse((root / policy["remove"][0]["path"]).exists())
            for entry in policy["edit"]:
                path = root / entry["path"]
                self.assertEqual(path.read_bytes(), b"keep\nkeep\n")
                self.assertEqual(path.stat().st_mode & 0o777, 0o755)

    def test_any_changed_source_refuses_before_deletion(self):
        for changed in range(3):
            with self.subTest(changed=changed), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                policy = self.fixture(root)
                selected = (policy["remove"] + policy["edit"])[changed]
                (root / selected["path"]).write_bytes(b"changed bytes")
                with self.assertRaises(ValueError):
                    prepare(root, policy)
                self.assertTrue((root / policy["remove"][0]["path"]).exists())
                for entry in policy["edit"]:
                    if entry is not selected:
                        self.assertEqual((root / entry["path"]).read_bytes(), b"keep\nremove\nkeep\n")

    def test_changed_edit_hash_and_escape_refuse(self):
        for changed in ("hash", "path", "duplicate"):
            with self.subTest(changed=changed), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                policy = self.fixture(root)
                broken = copy.deepcopy(policy)
                if changed == "hash":
                    broken["edit"][0]["remove"][0]["sha256"] = "0" * 64
                elif changed == "path":
                    broken["edit"][0]["path"] = "usr/share/omarchy/../../../outside"
                else:
                    broken["edit"][0]["path"] = broken["edit"][1]["path"]
                with self.assertRaises(ValueError):
                    prepare(root, broken)
                self.assertTrue((root / policy["remove"][0]["path"]).exists())


if __name__ == "__main__":
    unittest.main()
