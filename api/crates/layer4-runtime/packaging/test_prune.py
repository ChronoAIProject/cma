"""Packaging removes disposable bytes while preserving tools and root metadata."""
import json
import os
import tempfile
import unittest
from pathlib import Path

from prune import load_policy, prune


class PruneTests(unittest.TestCase):
    def test_development_files_removed_and_runtime_tools_preserved(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("tmp/node-compile-cache/data", "usr/local/lib/node_modules/npm/docs/example",
                         "usr/local/lib/node_modules/tool/test/example", "usr/local/lib/node_modules/tool/main.js",
                         "usr/lib/libcrypto.so", "usr/local/bin/nyxid"):
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b"synthetic payload")
            (root / "tmp").chmod(0o1777)
            policy = load_policy(Path(__file__).with_name("prune-policy.json"))
            self.assertGreater(prune(root, policy), 0)
            self.assertEqual(list((root / "tmp").iterdir()), [])
            self.assertEqual((root / "tmp").stat().st_mode & 0o7777, 0o1777)
            self.assertEqual(list((root / "usr/local/lib/node_modules/tool/test").iterdir()), [])
            for name in ("usr/local/lib/node_modules/tool/main.js", "usr/lib/libcrypto.so", "usr/local/bin/nyxid"):
                self.assertEqual((root / name).read_bytes(), b"synthetic payload")

    def test_symlinked_ancestor_refuses_before_any_deletion(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "root"
            root.mkdir()
            external = Path(directory) / "outside"
            external.mkdir()
            (external / "keep").write_bytes(b"preserve")
            (root / "tmp").mkdir()
            (root / "tmp/keep").write_bytes(b"preserve")
            os.symlink(external, root / "usr")
            policy = load_policy(Path(__file__).with_name("prune-policy.json"))
            with self.assertRaises(ValueError):
                prune(root, policy)
            self.assertTrue((root / "tmp/keep").exists())
            self.assertTrue((external / "keep").exists())

    def test_invalid_policy_refuses_paths_and_unknown_fields(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "policy.json"
            original = load_policy(Path(__file__).with_name("prune-policy.json"))
            for changes in ({"remove": ["../outside"]}, {"remove": ["/"]}, {"extra": True},
                            {"remove": ["tmp", "tmp"]}, {"development_directories": ["nested/tests"]}):
                path.write_text(json.dumps(dict(original, **changes)))
                with self.assertRaises(ValueError):
                    load_policy(path)


if __name__ == "__main__":
    unittest.main()
